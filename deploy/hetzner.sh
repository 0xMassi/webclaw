#!/usr/bin/env bash
# deploy/hetzner.sh — One-click Hetzner VPS deployment for webclaw
#
# Creates a Hetzner Cloud VPS with Docker, deploys webclaw + Ollama,
# and optionally configures nginx + SSL.
#
# Usage:
#   ./deploy/hetzner.sh              # Interactive setup
#   ./deploy/hetzner.sh --destroy    # Tear down the server
#
# Server type recommendations:
#   cpx11: 2 vCPU, 2GB RAM,  ~4.59 EUR/mo  — Minimum (scraping only, no LLM)
#   cpx21: 3 vCPU, 4GB RAM,  ~8.49 EUR/mo  — Recommended (scraping + small LLM)
#   cpx31: 4 vCPU, 8GB RAM,  ~15.59 EUR/mo — Best (scraping + LLM + high concurrency)
#   cpx41: 8 vCPU, 16GB RAM, ~28.19 EUR/mo — Heavy use (high-volume crawling)

set -euo pipefail

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
HETZNER_API="https://api.hetzner.cloud/v1"
SERVER_NAME="webclaw"
REPO_URL="https://github.com/0xMassi/webclaw.git"

# ---------------------------------------------------------------------------
# Colors
# ---------------------------------------------------------------------------
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
RESET='\033[0m'

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()    { printf "${BLUE}[*]${RESET} %s\n" "$*"; }
success() { printf "${GREEN}[+]${RESET} %s\n" "$*"; }
warn()    { printf "${YELLOW}[!]${RESET} %s\n" "$*"; }
error()   { printf "${RED}[x]${RESET} %s\n" "$*" >&2; }
fatal()   { error "$*"; exit 1; }

# Mask a secret for display: keep the last 4 chars, redact the rest.
# Empty input renders as "(not set)".
mask_secret() {
    local s="$1"
    if [[ -z "$s" ]]; then
        printf '(not set)'
    elif (( ${#s} <= 4 )); then
        printf '****'
    else
        printf '****%s' "${s: -4}"
    fi
}

prompt() {
    local var_name="$1" prompt_text="$2" default="${3:-}"
    if [[ -n "$default" ]]; then
        printf "${CYAN}    %s${DIM} [%s]${RESET}: " "$prompt_text" "$default"
    else
        printf "${CYAN}    %s${RESET}: " "$prompt_text"
    fi
    read -r input
    printf -v "$var_name" '%s' "${input:-$default}"
}

prompt_secret() {
    local var_name="$1" prompt_text="$2" default="${3:-}"
    if [[ -n "$default" ]]; then
        printf "${CYAN}    %s${DIM} [%s]${RESET}: " "$prompt_text" "$default"
    else
        printf "${CYAN}    %s${RESET}: " "$prompt_text"
    fi
    read -rs input
    echo
    printf -v "$var_name" '%s' "${input:-$default}"
}

generate_key() {
    # 32-char random hex key
    if command -v openssl &>/dev/null; then
        openssl rand -hex 16
    else
        LC_ALL=C tr -dc 'a-f0-9' < /dev/urandom | head -c 32
    fi
}

hetzner_api() {
    local method="$1" path="$2"
    shift 2
    curl -sf -X "$method" \
        -H "Authorization: Bearer $HETZNER_TOKEN" \
        -H "Content-Type: application/json" \
        "$HETZNER_API$path" \
        "$@"
}

# ---------------------------------------------------------------------------
# Preflight checks
# ---------------------------------------------------------------------------
preflight() {
    local missing=()
    command -v curl &>/dev/null || missing+=("curl")
    command -v jq   &>/dev/null || missing+=("jq")
    command -v ssh  &>/dev/null || missing+=("ssh")

    if [[ ${#missing[@]} -gt 0 ]]; then
        fatal "Missing required tools: ${missing[*]}. Install them and try again."
    fi
}

# ---------------------------------------------------------------------------
# Validate Hetzner token
# ---------------------------------------------------------------------------
validate_token() {
    info "Validating Hetzner API token..."
    local response
    response=$(hetzner_api GET "/servers?per_page=1" 2>&1) || {
        fatal "Invalid Hetzner API token. Get one at: https://console.hetzner.cloud"
    }
    success "Token is valid."
}

# ---------------------------------------------------------------------------
# Check if server already exists
# ---------------------------------------------------------------------------
find_server() {
    local response
    response=$(hetzner_api GET "/servers?name=$SERVER_NAME")
    echo "$response" | jq -r '.servers[0] // empty'
}

get_server_id() {
    local server
    server=$(find_server)
    if [[ -n "$server" && "$server" != "null" ]]; then
        echo "$server" | jq -r '.id'
    fi
}

get_server_ip() {
    local server
    server=$(find_server)
    if [[ -n "$server" && "$server" != "null" ]]; then
        echo "$server" | jq -r '.public_net.ipv4.ip'
    fi
}

# ---------------------------------------------------------------------------
# Destroy mode
# ---------------------------------------------------------------------------
destroy_server() {
    info "Looking for existing webclaw server..."
    local server_id
    server_id=$(get_server_id)

    if [[ -z "$server_id" ]]; then
        warn "No server named '$SERVER_NAME' found. Nothing to destroy."
        exit 0
    fi

    local ip
    ip=$(get_server_ip)
    warn "Found server: $SERVER_NAME (ID: $server_id, IP: $ip)"
    printf "${RED}    This will permanently delete the server and all its data.${RESET}\n"
    printf "${CYAN}    Type 'destroy' to confirm${RESET}: "
    read -r confirmation

    if [[ "$confirmation" != "destroy" ]]; then
        info "Aborted."
        exit 0
    fi

    info "Destroying server $server_id..."
    hetzner_api DELETE "/servers/$server_id" > /dev/null
    success "Server destroyed."

    # Clean SSH known_hosts
    if [[ -n "$ip" ]]; then
        ssh-keygen -R "$ip" 2>/dev/null || true
        info "Removed $ip from SSH known_hosts."
    fi
}

# ---------------------------------------------------------------------------
# Build cloud-init user_data
# ---------------------------------------------------------------------------
build_cloud_init() {
    local auth_key="$1" openai_key="$2" anthropic_key="$3" domain="$4" ollama_model="$5"

    # Build .env content
    local env_content="# webclaw deployment — generated by hetzner.sh
WEBCLAW_HOST=0.0.0.0
WEBCLAW_PORT=3000
WEBCLAW_API_KEY=$auth_key
OLLAMA_HOST=http://ollama:11434
OLLAMA_MODEL=$ollama_model
WEBCLAW_LOG=info"

    if [[ -n "$openai_key" ]]; then
        env_content="$env_content
OPENAI_API_KEY=$openai_key"
    fi
    if [[ -n "$anthropic_key" ]]; then
        env_content="$env_content
ANTHROPIC_API_KEY=$anthropic_key"
    fi

    # Nginx + certbot block (only if domain provided)
    local nginx_block=""
    if [[ -n "$domain" ]]; then
        nginx_block="
# --- Nginx reverse proxy + SSL ---
- apt-get install -y nginx certbot python3-certbot-nginx

- |
  cat > /etc/nginx/sites-available/webclaw <<'NGINX'
  server {
      listen 80;
      server_name $domain;

      location / {
          proxy_pass http://127.0.0.1:3000;
          proxy_set_header Host \$host;
          proxy_set_header X-Real-IP \$remote_addr;
          proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
          proxy_set_header X-Forwarded-Proto \$scheme;
          proxy_read_timeout 120s;
          proxy_connect_timeout 10s;
      }
  }
  NGINX

- ln -sf /etc/nginx/sites-available/webclaw /etc/nginx/sites-enabled/webclaw
- rm -f /etc/nginx/sites-enabled/default
- systemctl restart nginx

# SSL cert (will fail silently if DNS not pointed yet)
- certbot --nginx -d $domain --non-interactive --agree-tos --register-unsolicited-contact -m admin@$domain || echo 'Certbot failed — point DNS to this IP and run: certbot --nginx -d $domain'
"
    fi

    cat <<CLOUDINIT
#cloud-config
package_update: true

runcmd:
# --- Firewall ---
- ufw allow 22/tcp
- ufw allow 80/tcp
- ufw allow 443/tcp
- ufw allow 3000/tcp
- ufw --force enable

# --- Docker (already installed on hetzner docker-ce image, but ensure compose) ---
- |
  if ! command -v docker &>/dev/null; then
    curl -fsSL https://get.docker.com | sh
  fi
- |
  if ! docker compose version &>/dev/null; then
    apt-get install -y docker-compose-plugin
  fi

# --- Clone and deploy ---
- git clone $REPO_URL /opt/webclaw
- |
  cat > /opt/webclaw/.env <<'DOTENV'
  $env_content
  DOTENV
  # Remove leading whitespace from heredoc
  sed -i 's/^  //' /opt/webclaw/.env

$nginx_block
# --- Start services ---
- cd /opt/webclaw && docker compose up -d --build

# --- Pull Ollama model in background (non-blocking) ---
- |
  nohup bash -c '
    echo "Waiting for Ollama to start..."
    for i in \$(seq 1 60); do
      if docker compose -f /opt/webclaw/docker-compose.yml exec -T ollama ollama list &>/dev/null; then
        echo "Ollama ready. Pulling $ollama_model..."
        docker compose -f /opt/webclaw/docker-compose.yml exec -T ollama ollama pull $ollama_model
        echo "Model $ollama_model pulled."
        break
      fi
      sleep 5
    done
  ' > /var/log/ollama-pull.log 2>&1 &

CLOUDINIT
}

# ---------------------------------------------------------------------------
# Wait for SSH
# ---------------------------------------------------------------------------
wait_for_ssh() {
    local ip="$1" max_attempts=40
    info "Waiting for server to become reachable (this takes 1-3 minutes)..."

    for i in $(seq 1 $max_attempts); do
        if ssh -o ConnectTimeout=3 -o StrictHostKeyChecking=no -o BatchMode=yes \
            "root@$ip" "echo ok" &>/dev/null; then
            return 0
        fi
        printf "."
        sleep 5
    done
    echo
    return 1
}

# ---------------------------------------------------------------------------
# Wait for Docker build to complete
# ---------------------------------------------------------------------------
wait_for_docker() {
    local ip="$1" max_attempts=60
    info "Waiting for Docker build to complete (this takes 5-15 minutes on first deploy)..."

    for i in $(seq 1 $max_attempts); do
        local status
        status=$(ssh -o ConnectTimeout=5 -o StrictHostKeyChecking=no \
            "root@$ip" "docker ps --filter name=webclaw --format '{{.Status}}' 2>/dev/null | head -1" 2>/dev/null || echo "")

        if [[ "$status" == *"Up"* ]]; then
            return 0
        fi
        printf "."
        sleep 15
    done
    echo
    return 1
}

# ---------------------------------------------------------------------------
# Get SSH keys from Hetzner account
# ---------------------------------------------------------------------------
get_ssh_keys() {
    local response
    response=$(hetzner_api GET "/ssh_keys")
    echo "$response" | jq -r '[.ssh_keys[].id] // []'
}

# ---------------------------------------------------------------------------
# Main: create server
# ---------------------------------------------------------------------------
create_server() {
    # Check for existing server
    local existing_id
    existing_id=$(get_server_id)
    if [[ -n "$existing_id" ]]; then
        local existing_ip
        existing_ip=$(get_server_ip)
        warn "Server '$SERVER_NAME' already exists (ID: $existing_id, IP: $existing_ip)"
        warn "Run with --destroy first, or use a different name."
        exit 1
    fi

    # Gather configuration
    echo
    printf "${BOLD}${GREEN}  webclaw Hetzner Deploy${RESET}\n"
    printf "${DIM}  One-click VPS deployment for webclaw REST API + Ollama${RESET}\n"
    echo

    prompt      SERVER_TYPE "Server type (cpx11/cpx21/cpx31/cpx41)" "cpx21"
    prompt      LOCATION    "Region (fsn1/nbg1/hel1/ash/hil)" "fsn1"
    prompt      DOMAIN      "Domain for SSL (leave empty to skip)" ""
    prompt_secret OPENAI_KEY  "OpenAI API key (optional)" ""
    prompt_secret ANTHROPIC_KEY "Anthropic API key (optional)" ""

    local generated_auth_key
    generated_auth_key=$(generate_key)
    prompt_secret AUTH_KEY "Webclaw auth key" "$generated_auth_key"

    prompt OLLAMA_MODEL "Ollama model to pre-pull" "qwen3:1.7b"

    echo
    info "Configuration:"
    printf "    Server type:  ${BOLD}%s${RESET}\n" "$SERVER_TYPE"
    printf "    Region:       ${BOLD}%s${RESET}\n" "$LOCATION"
    printf "    Domain:       ${BOLD}%s${RESET}\n" "${DOMAIN:-none}"
    printf "    OpenAI key:   ${BOLD}%s${RESET}\n" "$([ -n "$OPENAI_KEY" ] && echo 'set' || echo 'not set')"
    printf "    Anthropic key:${BOLD}%s${RESET}\n" "$([ -n "$ANTHROPIC_KEY" ] && echo 'set' || echo 'not set')"
    printf "    Auth key:     ${BOLD}%s${RESET}\n" "$(mask_secret "$AUTH_KEY")"
    printf "    Ollama model: ${BOLD}%s${RESET}\n" "$OLLAMA_MODEL"
    echo

    printf "${CYAN}    Proceed? (y/n)${RESET}: "
    read -r confirm
    [[ "$confirm" =~ ^[Yy]$ ]] || { info "Aborted."; exit 0; }

    # Build cloud-init
    local user_data
    user_data=$(build_cloud_init "$AUTH_KEY" "$OPENAI_KEY" "$ANTHROPIC_KEY" "$DOMAIN" "$OLLAMA_MODEL")

    # Get SSH keys
    local ssh_keys
    ssh_keys=$(get_ssh_keys)
    info "Found $(echo "$ssh_keys" | jq length) SSH key(s) in your Hetzner account."

    # Create server
    info "Creating $SERVER_TYPE server in $LOCATION..."
    local create_payload
    create_payload=$(jq -n \
        --arg name "$SERVER_NAME" \
        --arg server_type "$SERVER_TYPE" \
        --arg location "$LOCATION" \
        --arg user_data "$user_data" \
        --argjson ssh_keys "$ssh_keys" \
        '{
            name: $name,
            server_type: $server_type,
            location: $location,
            image: "docker-ce",
            ssh_keys: $ssh_keys,
            user_data: $user_data,
            public_net: {
                enable_ipv4: true,
                enable_ipv6: true
            }
        }')

    local response
    response=$(hetzner_api POST "/servers" -d "$create_payload") || {
        fatal "Failed to create server. Check your Hetzner token permissions."
    }

    local server_id server_ip root_password
    server_id=$(echo "$response" | jq -r '.server.id')
    server_ip=$(echo "$response" | jq -r '.server.public_net.ipv4.ip')
    root_password=$(echo "$response" | jq -r '.root_password // empty')

    if [[ -z "$server_id" || "$server_id" == "null" ]]; then
        error "Server creation response:"
        echo "$response" | jq .
        fatal "Failed to create server."
    fi

    success "Server created: ID=$server_id, IP=$server_ip"

    if [[ -n "$root_password" ]]; then
        echo
        warn "Root password (save this, shown only once): $root_password"
        echo
    fi

    # Wait for SSH
    if wait_for_ssh "$server_ip"; then
        success "Server is reachable via SSH."
    else
        warn "Server not yet reachable via SSH. It may still be booting."
        warn "Try: ssh root@$server_ip"
    fi

    # Summary
    echo
    printf "${BOLD}${GREEN}  Deployment started.${RESET}\n"
    echo
    printf "  The server is now building webclaw from source.\n"
    printf "  This takes ${BOLD}5-15 minutes${RESET} on first deploy.\n"
    echo
    printf "  ${BOLD}Server IP:${RESET}    %s\n" "$server_ip"
    printf "  ${BOLD}SSH:${RESET}          ssh root@%s\n" "$server_ip"
    printf "  ${BOLD}Auth key:${RESET}     %s\n" "$(mask_secret "$AUTH_KEY")"
    printf "  ${DIM}(full key stored in /opt/webclaw/.env on the server:\n"
    printf "   ssh root@%s 'grep WEBCLAW_API_KEY /opt/webclaw/.env')${RESET}\n" "$server_ip"
    echo
    printf "  ${BOLD}Monitor build progress:${RESET}\n"
    printf "    ssh root@%s 'cd /opt/webclaw && docker compose logs -f'\n" "$server_ip"
    echo
    printf "  ${BOLD}Test when ready:${RESET}\n"
    printf "    curl http://%s:3000/health\n" "$server_ip"
    echo
    printf "  ${BOLD}Scrape:${RESET}\n"
    printf "    curl -X POST http://%s:3000/v1/scrape \\\\\n" "$server_ip"
    printf "      -H 'Content-Type: application/json' \\\\\n"
    printf "      -H 'Authorization: Bearer <YOUR_AUTH_KEY>' \\\\\n"
    printf "      -d '{\"url\": \"https://example.com\"}'\n"
    echo

    if [[ -n "$DOMAIN" ]]; then
        printf "  ${BOLD}Domain:${RESET}\n"
        printf "    Point %s A record -> %s\n" "$DOMAIN" "$server_ip"
        printf "    SSL will auto-configure via certbot.\n"
        printf "    Then: curl https://%s/health\n" "$DOMAIN"
        echo
    fi

    printf "  ${BOLD}Pull Ollama model manually (if auto-pull hasn't finished):${RESET}\n"
    printf "    ssh root@%s 'cd /opt/webclaw && docker compose exec ollama ollama pull %s'\n" "$server_ip" "$OLLAMA_MODEL"
    echo

    printf "  ${BOLD}Tear down:${RESET}\n"
    printf "    HETZNER_TOKEN=\$HETZNER_TOKEN ./deploy/hetzner.sh --destroy\n"
    printf "    ${DIM}(re-export the same token you used to deploy)${RESET}\n"
    echo
}

# ---------------------------------------------------------------------------
# Entrypoint
# ---------------------------------------------------------------------------
main() {
    preflight

    # Accept token from env or prompt
    if [[ -z "${HETZNER_TOKEN:-}" ]]; then
        echo
        printf "${BOLD}${GREEN}  webclaw Hetzner Deploy${RESET}\n"
        echo
        prompt_secret HETZNER_TOKEN "Hetzner API token (https://console.hetzner.cloud)" ""
        [[ -n "$HETZNER_TOKEN" ]] || fatal "Hetzner API token is required."
    fi

    validate_token

    if [[ "${1:-}" == "--destroy" ]]; then
        destroy_server
    else
        create_server
    fi
}

main "$@"
