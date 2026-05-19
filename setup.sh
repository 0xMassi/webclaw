#!/usr/bin/env bash
# setup.sh — Local setup for webclaw
#
# Checks prerequisites, builds binaries, configures .env,
# optionally installs Ollama, and wires up the MCP server.
#
# Usage:
#   ./setup.sh              # Interactive full setup
#   ./setup.sh --minimal    # Build only, skip configuration
#   ./setup.sh --check      # Check prerequisites without installing

set -euo pipefail

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

info()    { printf "${BLUE}[*]${RESET} %s\n" "$*"; }
success() { printf "${GREEN}[+]${RESET} %s\n" "$*"; }
warn()    { printf "${YELLOW}[!]${RESET} %s\n" "$*"; }
error()   { printf "${RED}[x]${RESET} %s\n" "$*" >&2; }

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

prompt_yn() {
    local prompt_text="$1" default="${2:-y}"
    local hint="Y/n"
    [[ "$default" == "n" ]] && hint="y/N"
    printf "${CYAN}    %s${DIM} [%s]${RESET}: " "$prompt_text" "$hint"
    read -r input
    input="${input:-$default}"
    [[ "$input" =~ ^[Yy]$ ]]
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ---------------------------------------------------------------------------
# Step 1: Check prerequisites
# ---------------------------------------------------------------------------
check_prerequisites() {
    echo
    printf "${BOLD}${GREEN}  Step 1: Prerequisites${RESET}\n"
    echo

    local all_good=true

    # Rust
    if command -v rustc &>/dev/null; then
        local rust_version
        rust_version=$(rustc --version | awk '{print $2}')
        success "Rust $rust_version"

        # Check minimum version (1.85 for edition 2024)
        local major minor
        major=$(echo "$rust_version" | cut -d. -f1)
        minor=$(echo "$rust_version" | cut -d. -f2)
        if [[ "$major" -lt 1 ]] || [[ "$major" -eq 1 && "$minor" -lt 85 ]]; then
            warn "Rust 1.85+ required (edition 2024). Run: rustup update"
            all_good=false
        fi
    else
        error "Rust not found. Install: https://rustup.rs"
        all_good=false
    fi

    # cargo
    if command -v cargo &>/dev/null; then
        success "Cargo $(cargo --version | awk '{print $2}')"
    else
        error "Cargo not found (should come with Rust)"
        all_good=false
    fi

    # Ollama (optional)
    if command -v ollama &>/dev/null; then
        success "Ollama installed"
        if curl -sf http://localhost:11434/api/tags &>/dev/null; then
            success "Ollama is running"
            local models
            models=$(curl -sf http://localhost:11434/api/tags | python3 -c "import sys,json; [print(m['name']) for m in json.load(sys.stdin).get('models',[])]" 2>/dev/null || echo "")
            if [[ -n "$models" ]]; then
                success "Models: $(echo "$models" | tr '\n' ', ' | sed 's/,$//')"
            else
                warn "No models pulled yet"
            fi
        else
            warn "Ollama installed but not running. Start with: ollama serve"
        fi
    else
        warn "Ollama not found (optional — needed for local LLM features)"
    fi

    # Git
    if command -v git &>/dev/null; then
        success "Git $(git --version | awk '{print $3}')"
    else
        error "Git not found"
        all_good=false
    fi

    echo
    if $all_good; then
        success "All prerequisites met."
    else
        error "Some prerequisites are missing. Fix them before continuing."
        [[ "${1:-}" == "--check" ]] && exit 1
    fi
}

# ---------------------------------------------------------------------------
# Step 2: Build
# ---------------------------------------------------------------------------
build_binaries() {
    echo
    printf "${BOLD}${GREEN}  Step 2: Build${RESET}\n"
    echo

    info "Building release binaries (this may take a few minutes on first build)..."
    cd "$SCRIPT_DIR"

    if cargo build --release 2>&1 | tail -5; then
        echo
        success "Built 3 binaries:"
        ls -lh target/release/webclaw target/release/webclaw-server target/release/webclaw-mcp 2>/dev/null | \
            awk '{printf "    %-20s %s\n", $NF, $5}'
    else
        error "Build failed. Check the output above."
        exit 1
    fi
}

# ---------------------------------------------------------------------------
# Step 3: Configure .env
# ---------------------------------------------------------------------------
configure_env() {
    echo
    printf "${BOLD}${GREEN}  Step 3: Configuration${RESET}\n"
    echo

    if [[ -f "$SCRIPT_DIR/.env" ]]; then
        warn ".env already exists."
        if ! prompt_yn "Overwrite?"; then
            info "Keeping existing .env"
            return
        fi
    fi

    local ollama_model="qwen3:8b"
    local openai_key=""
    local anthropic_key=""
    local proxy_file=""
    local server_port="3000"
    local auth_key=""

    info "LLM configuration"
    prompt ollama_model "Ollama model (local)" "qwen3:8b"
    prompt_secret openai_key "OpenAI API key (optional, press enter to skip)" ""
    prompt_secret anthropic_key "Anthropic API key (optional, press enter to skip)" ""

    echo
    info "Proxy configuration"
    if [[ -f "$SCRIPT_DIR/proxies.txt" ]]; then
        local proxy_count
        proxy_count=$(grep -cv '^\s*#\|^\s*$' "$SCRIPT_DIR/proxies.txt" 2>/dev/null || echo "0")
        success "proxies.txt found with $proxy_count proxies (auto-loaded)"
    else
        info "To use proxies, create proxies.txt with one proxy per line:"
        printf "    ${DIM}Format: host:port:user:pass${RESET}\n"
        printf "    ${DIM}cp proxies.example.txt proxies.txt${RESET}\n"
    fi
    local proxy_file=""

    echo
    info "Server configuration"
    prompt server_port "REST API port" "3000"
    prompt auth_key "API auth key (press enter to auto-generate)" ""

    if [[ -z "$auth_key" ]]; then
        if command -v openssl &>/dev/null; then
            auth_key=$(openssl rand -hex 16)
        else
            auth_key=$(LC_ALL=C tr -dc 'a-f0-9' < /dev/urandom | head -c 32)
        fi
        info "Generated auth key: $auth_key"
    fi

    # Write .env
    cat > "$SCRIPT_DIR/.env" <<EOF
# webclaw configuration — generated by setup.sh

# --- LLM Providers ---
OLLAMA_HOST=http://localhost:11434
OLLAMA_MODEL=$ollama_model
EOF

    if [[ -n "$openai_key" ]]; then
        echo "OPENAI_API_KEY=$openai_key" >> "$SCRIPT_DIR/.env"
    fi
    if [[ -n "$anthropic_key" ]]; then
        echo "ANTHROPIC_API_KEY=$anthropic_key" >> "$SCRIPT_DIR/.env"
    fi

    cat >> "$SCRIPT_DIR/.env" <<EOF

# --- Proxy ---
EOF
    if [[ -n "$proxy_file" ]]; then
        echo "WEBCLAW_PROXY_FILE=$proxy_file" >> "$SCRIPT_DIR/.env"
    else
        echo "# WEBCLAW_PROXY_FILE=/path/to/proxies.txt" >> "$SCRIPT_DIR/.env"
    fi

    cat >> "$SCRIPT_DIR/.env" <<EOF

# --- Server ---
WEBCLAW_PORT=$server_port
WEBCLAW_HOST=0.0.0.0
WEBCLAW_AUTH_KEY=$auth_key

# --- Logging ---
WEBCLAW_LOG=info
EOF

    echo
    success ".env created."
}

# ---------------------------------------------------------------------------
# Step 4: Install Ollama (optional)
# ---------------------------------------------------------------------------
setup_ollama() {
    echo
    printf "${BOLD}${GREEN}  Step 4: Ollama (Local LLM)${RESET}\n"
    echo

    if ! command -v ollama &>/dev/null; then
        info "Ollama is not installed."
        info "It's optional but needed for local LLM features (extract, summarize)."
        info "Without it, you can still use OpenAI/Anthropic APIs."
        echo
        if prompt_yn "Install Ollama?" "y"; then
            info "Installing Ollama..."
            if [[ "$(uname)" == "Darwin" ]]; then
                if command -v brew &>/dev/null; then
                    brew install ollama
                else
                    warn "Install Ollama manually: https://ollama.ai/download"
                    return
                fi
            else
                curl -fsSL https://ollama.ai/install.sh | sh
            fi
            success "Ollama installed."
        else
            info "Skipping Ollama. You can install later: https://ollama.ai"
            return
        fi
    fi

    # Check if running
    if ! curl -sf http://localhost:11434/api/tags &>/dev/null; then
        warn "Ollama is not running."
        if [[ "$(uname)" == "Darwin" ]]; then
            info "On macOS, open the Ollama app or run: ollama serve"
        else
            info "Start with: ollama serve"
        fi
        echo
        if prompt_yn "Start Ollama now?" "y"; then
            nohup ollama serve &>/dev/null &
            sleep 2
            if curl -sf http://localhost:11434/api/tags &>/dev/null; then
                success "Ollama is running."
            else
                warn "Ollama didn't start. Start it manually and re-run setup."
                return
            fi
        else
            return
        fi
    fi

    # Pull model
    local model
    model=$(grep '^OLLAMA_MODEL=' "$SCRIPT_DIR/.env" 2>/dev/null | cut -d= -f2 || echo "qwen3:8b")

    local has_model
    has_model=$(curl -sf http://localhost:11434/api/tags | python3 -c "import sys,json; models=[m['name'] for m in json.load(sys.stdin).get('models',[])]; print('yes' if any('$model' in m for m in models) else 'no')" 2>/dev/null || echo "no")

    if [[ "$has_model" == "yes" ]]; then
        success "Model $model already available."
    else
        info "Model $model not found locally."
        if prompt_yn "Pull $model now? (this downloads ~5GB)" "y"; then
            ollama pull "$model"
            success "Model $model ready."
        fi
    fi
}

# ---------------------------------------------------------------------------
# Step 5: Configure MCP server for Claude Desktop
# ---------------------------------------------------------------------------
setup_mcp() {
    echo
    printf "${BOLD}${GREEN}  Step 5: MCP Server (Claude Desktop integration)${RESET}\n"
    echo

    local mcp_binary="$SCRIPT_DIR/target/release/webclaw-mcp"
    if [[ ! -f "$mcp_binary" ]]; then
        warn "webclaw-mcp binary not found. Build first."
        return
    fi

    info "The MCP server lets Claude Desktop use webclaw's tools directly."
    info "Tools: scrape, crawl, map, batch, extract, summarize, diff, brand"
    echo

    if ! prompt_yn "Configure MCP server for Claude Desktop?" "y"; then
        info "Skipping MCP setup."
        info "You can configure it later by adding to your Claude Desktop config:"
        printf '    {"mcpServers": {"webclaw": {"command": "%s"}}}\n' "$mcp_binary"
        return
    fi

    # Find Claude Desktop config
    local config_path=""
    if [[ "$(uname)" == "Darwin" ]]; then
        config_path="$HOME/Library/Application Support/Claude/claude_desktop_config.json"
    else
        config_path="$HOME/.config/claude/claude_desktop_config.json"
    fi

    if [[ ! -f "$config_path" ]]; then
        # Create config directory and file
        mkdir -p "$(dirname "$config_path")"
        echo '{}' > "$config_path"
        info "Created Claude Desktop config at: $config_path"
    fi

    # Read existing config and merge
    local existing
    existing=$(cat "$config_path")

    # Check if webclaw is already configured
    if echo "$existing" | python3 -c "import sys,json; c=json.load(sys.stdin); exit(0 if 'webclaw' in c.get('mcpServers',{}) else 1)" 2>/dev/null; then
        warn "webclaw MCP server already configured in Claude Desktop."
        if ! prompt_yn "Update the path?" "y"; then
            return
        fi
    fi

    # Merge webclaw into mcpServers
    local updated
    updated=$(echo "$existing" | python3 -c "
import sys, json
config = json.load(sys.stdin)
if 'mcpServers' not in config:
    config['mcpServers'] = {}
config['mcpServers']['webclaw'] = {
    'command': '$mcp_binary'
}
print(json.dumps(config, indent=2))
")

    echo "$updated" > "$config_path"
    success "MCP server configured in Claude Desktop."
    info "Restart Claude Desktop to activate."
}

# ---------------------------------------------------------------------------
# Step 6: Smoke test
# ---------------------------------------------------------------------------
smoke_test() {
    echo
    printf "${BOLD}${GREEN}  Step 6: Smoke Test${RESET}\n"
    echo

    local webclaw="$SCRIPT_DIR/target/release/webclaw"

    info "Testing extraction..."
    local output
    output=$("$webclaw" https://example.com --format llm 2>/dev/null || echo "FAILED")

    if [[ "$output" == "FAILED" ]]; then
        warn "Extraction test failed. Check your network connection."
    else
        local word_count
        word_count=$(echo "$output" | wc -w | tr -d ' ')
        success "Extracted example.com: $word_count words"
    fi

    # Test Ollama if available
    if curl -sf http://localhost:11434/api/tags &>/dev/null; then
        info "Testing LLM summarization..."
        local summary
        summary=$("$webclaw" https://example.com --summarize 2>/dev/null || echo "FAILED")
        if [[ "$summary" == "FAILED" ]]; then
            warn "LLM test failed. Check Ollama and model availability."
        else
            success "LLM summarization works."
        fi
    fi
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
print_summary() {
    local webclaw="$SCRIPT_DIR/target/release/webclaw"
    local server="$SCRIPT_DIR/target/release/webclaw-server"
    local mcp="$SCRIPT_DIR/target/release/webclaw-mcp"
    local port
    port=$(grep '^WEBCLAW_PORT=' "$SCRIPT_DIR/.env" 2>/dev/null | cut -d= -f2 || echo "3000")

    echo
    printf "${BOLD}${GREEN}  Setup Complete${RESET}\n"
    echo
    printf "  ${BOLD}CLI:${RESET}\n"
    printf "    %s https://example.com --format llm\n" "$webclaw"
    echo
    printf "  ${BOLD}REST API:${RESET}\n"
    printf "    %s\n" "$server"
    printf "    curl http://localhost:%s/health\n" "$port"
    echo
    printf "  ${BOLD}MCP Server:${RESET}\n"
    printf "    Configured in Claude Desktop (restart to activate)\n"
    echo
    printf "  ${BOLD}Config:${RESET}  %s/.env\n" "$SCRIPT_DIR"
    printf "  ${BOLD}Docs:${RESET}    %s/README.md\n" "$SCRIPT_DIR"
    echo
    printf "  ${DIM}Tip: Add to PATH for convenience:${RESET}\n"
    printf "    export PATH=\"%s/target/release:\$PATH\"\n" "$SCRIPT_DIR"
    echo
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    echo
    printf "${BOLD}${GREEN}  webclaw — Local Setup${RESET}\n"
    printf "${DIM}  Web extraction toolkit for AI agents${RESET}\n"
    echo

    local mode="${1:-}"

    if [[ "$mode" == "--check" ]]; then
        check_prerequisites "--check"
        exit 0
    fi

    check_prerequisites

    build_binaries

    if [[ "$mode" == "--minimal" ]]; then
        success "Minimal build complete. Run ./setup.sh for full configuration."
        exit 0
    fi

    configure_env
    setup_ollama
    setup_mcp
    smoke_test
    print_summary
}

main "$@"
