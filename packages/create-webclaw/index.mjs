#!/usr/bin/env node

import {
  existsSync,
  mkdirSync,
  readFileSync,
  writeFileSync,
  copyFileSync,
  rmSync,
} from "fs";
import { createInterface } from "readline";
import { homedir, platform, arch } from "os";
import { join, dirname } from "path";
import { execSync } from "child_process";
import { createWriteStream } from "fs";
import { chmod } from "fs/promises";
import https from "https";
import http from "http";

// ── Constants ──

const REPO = "0xMassi/webclaw";
const IS_WINDOWS = platform() === "win32";
const BINARY_NAME = IS_WINDOWS ? "webclaw-mcp.exe" : "webclaw-mcp";
const INSTALL_DIR = join(homedir(), ".webclaw");
const BINARY_PATH = join(INSTALL_DIR, BINARY_NAME);

const COLORS = {
  reset: "\x1b[0m",
  bold: "\x1b[1m",
  dim: "\x1b[2m",
  green: "\x1b[32m",
  yellow: "\x1b[33m",
  blue: "\x1b[34m",
  cyan: "\x1b[36m",
  red: "\x1b[31m",
};

const c = (color, text) => `${COLORS[color]}${text}${COLORS.reset}`;

// ── AI Tool Detection ──

const AI_TOOLS = [
  {
    id: "claude-desktop",
    name: "Claude Desktop",
    detect: () => {
      if (platform() === "darwin")
        return existsSync(
          join(
            homedir(),
            "Library/Application Support/Claude/claude_desktop_config.json",
          ),
        );
      if (platform() === "win32")
        return existsSync(
          join(process.env.APPDATA || "", "Claude/claude_desktop_config.json"),
        );
      return false;
    },
    configPath: () => {
      if (platform() === "darwin")
        return join(
          homedir(),
          "Library/Application Support/Claude/claude_desktop_config.json",
        );
      if (platform() === "win32")
        return join(
          process.env.APPDATA || "",
          "Claude/claude_desktop_config.json",
        );
      return null;
    },
  },
  {
    id: "claude-code",
    name: "Claude Code",
    detect: () => existsSync(join(homedir(), ".claude.json")),
    configPath: () => join(homedir(), ".claude.json"),
  },
  {
    id: "cursor",
    name: "Cursor",
    detect: () => {
      // Check for .cursor directory in home or current project
      return (
        existsSync(join(homedir(), ".cursor")) ||
        existsSync(join(process.cwd(), ".cursor"))
      );
    },
    configPath: () => {
      const projectPath = join(process.cwd(), ".cursor", "mcp.json");
      const globalPath = join(homedir(), ".cursor", "mcp.json");
      return existsSync(join(process.cwd(), ".cursor"))
        ? projectPath
        : globalPath;
    },
  },
  {
    id: "windsurf",
    name: "Windsurf",
    detect: () => {
      return (
        existsSync(join(homedir(), ".codeium")) ||
        existsSync(join(homedir(), ".windsurf"))
      );
    },
    configPath: () =>
      join(homedir(), ".codeium", "windsurf", "mcp_config.json"),
  },
  {
    id: "vscode-continue",
    name: "VS Code (Continue)",
    detect: () => existsSync(join(homedir(), ".continue")),
    configPath: () => join(homedir(), ".continue", "config.json"),
  },
  {
    id: "opencode",
    name: "OpenCode",
    detect: () => {
      return (
        existsSync(join(homedir(), ".config", "opencode", "opencode.json")) ||
        existsSync(join(process.cwd(), "opencode.json"))
      );
    },
    configPath: () => {
      const projectPath = join(process.cwd(), "opencode.json");
      const globalPath = join(
        homedir(),
        ".config",
        "opencode",
        "opencode.json",
      );
      return existsSync(projectPath) ? projectPath : globalPath;
    },
  },
  {
    id: "antigravity",
    name: "Antigravity",
    detect: () => {
      return (
        existsSync(join(homedir(), ".antigravity")) ||
        existsSync(join(homedir(), ".config", "antigravity"))
      );
    },
    configPath: () => {
      const configDir = existsSync(join(homedir(), ".config", "antigravity"))
        ? join(homedir(), ".config", "antigravity")
        : join(homedir(), ".antigravity");
      return join(configDir, "mcp.json");
    },
  },
  {
    id: "codex",
    name: "Codex (CLI + App)",
    detect: () => existsSync(join(homedir(), ".codex")),
    configPath: () => join(homedir(), ".codex", "config.toml"),
  },
];

// ── Helpers ──

function ask(question) {
  const rl = createInterface({
    input: process.stdin,
    output: process.stdout,
  });
  return new Promise((resolve) => {
    rl.question(question, (answer) => {
      rl.close();
      resolve(answer.trim());
    });
  });
}

function download(url, extraHeaders = {}) {
  return new Promise((resolve, reject) => {
    const client = url.startsWith("https") ? https : http;
    const headers = { "User-Agent": "create-webclaw", ...extraHeaders };
    client
      .get(url, { headers }, (res) => {
        // Follow redirects, dropping extra headers so an Authorization token
        // never leaks to the release CDN (its signed URLs reject it anyway).
        if (
          res.statusCode >= 300 &&
          res.statusCode < 400 &&
          res.headers.location
        ) {
          return download(res.headers.location).then(resolve).catch(reject);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`HTTP ${res.statusCode}`));
        }
        const chunks = [];
        res.on("data", (chunk) => chunks.push(chunk));
        res.on("end", () => resolve(Buffer.concat(chunks)));
        res.on("error", reject);
      })
      .on("error", reject);
  });
}

async function downloadFile(url, dest) {
  return new Promise((resolve, reject) => {
    const client = url.startsWith("https") ? https : http;
    client
      .get(url, { headers: { "User-Agent": "create-webclaw" } }, (res) => {
        if (
          res.statusCode >= 300 &&
          res.statusCode < 400 &&
          res.headers.location
        ) {
          return downloadFile(res.headers.location, dest)
            .then(resolve)
            .catch(reject);
        }
        if (res.statusCode !== 200) {
          return reject(new Error(`HTTP ${res.statusCode}`));
        }
        const file = createWriteStream(dest);
        res.pipe(file);
        file.on("finish", () => {
          file.close();
          resolve();
        });
        file.on("error", reject);
      })
      .on("error", reject);
  });
}

// Map the current platform to its Rust release target triple. Release assets
// are named `webclaw-<tag>-<target>.<ext>` (e.g.
// webclaw-v0.6.13-x86_64-unknown-linux-gnu.tar.gz), so the asset name is built
// from the release's tag_name at fetch time — it can't be hardcoded here.
function getTarget() {
  const targets = {
    "darwin-arm64": "aarch64-apple-darwin",
    "darwin-x64": "x86_64-apple-darwin",
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "win32-x64": "x86_64-pc-windows-msvc",
  };
  return targets[`${platform()}-${arch()}`] || null;
}

function readJsonFile(path) {
  try {
    return JSON.parse(readFileSync(path, "utf-8"));
  } catch {
    return {};
  }
}

function writeJsonFile(path, data) {
  const dir = dirname(path);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
  writeFileSync(path, JSON.stringify(data, null, 2) + "\n");
}

function buildMcpEntry(apiKey) {
  const entry = {
    command: BINARY_PATH,
  };
  if (apiKey) {
    entry.env = { WEBCLAW_API_KEY: apiKey };
  }
  return entry;
}

// ── MCP Config Writers ──

function addToClaudeDesktop(configPath, apiKey) {
  const config = readJsonFile(configPath);
  if (!config.mcpServers) config.mcpServers = {};
  config.mcpServers.webclaw = buildMcpEntry(apiKey);
  writeJsonFile(configPath, config);
}

function addToClaudeCode(configPath, apiKey) {
  const config = readJsonFile(configPath);
  if (!config.mcpServers) config.mcpServers = {};
  config.mcpServers.webclaw = buildMcpEntry(apiKey);
  writeJsonFile(configPath, config);
}

function addToCursor(configPath, apiKey) {
  const config = readJsonFile(configPath);
  if (!config.mcpServers) config.mcpServers = {};
  config.mcpServers.webclaw = {
    command: BINARY_PATH,
    ...(apiKey ? { env: { WEBCLAW_API_KEY: apiKey } } : {}),
  };
  writeJsonFile(configPath, config);
}

function addToWindsurf(configPath, apiKey) {
  const config = readJsonFile(configPath);
  if (!config.mcpServers) config.mcpServers = {};
  config.mcpServers.webclaw = buildMcpEntry(apiKey);
  writeJsonFile(configPath, config);
}

function addToVSCodeContinue(configPath, apiKey) {
  const config = readJsonFile(configPath);
  if (!config.mcpServers) config.mcpServers = [];
  // Continue uses array format
  const existing = config.mcpServers.findIndex?.((s) => s.name === "webclaw");
  const entry = {
    name: "webclaw",
    command: BINARY_PATH,
    ...(apiKey ? { env: { WEBCLAW_API_KEY: apiKey } } : {}),
  };
  if (existing >= 0) {
    config.mcpServers[existing] = entry;
  } else if (Array.isArray(config.mcpServers)) {
    config.mcpServers.push(entry);
  }
  writeJsonFile(configPath, config);
}

function addToOpenCode(configPath, apiKey) {
  const config = readJsonFile(configPath);
  if (!config.mcp) config.mcp = {};
  config.mcp.webclaw = {
    type: "local",
    command: [BINARY_PATH],
    enabled: true,
  };
  if (apiKey) {
    config.mcp.webclaw.environment = { WEBCLAW_API_KEY: apiKey };
  }
  writeJsonFile(configPath, config);
}

function addToAntigravity(configPath, apiKey) {
  const config = readJsonFile(configPath);
  if (!config.mcpServers) config.mcpServers = {};
  config.mcpServers.webclaw = buildMcpEntry(apiKey);
  writeJsonFile(configPath, config);
}

function addToCodex(configPath, apiKey) {
  // Codex uses TOML format, not JSON. Append MCP server config section.
  const dir = dirname(configPath);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });

  let existing = "";
  try {
    existing = readFileSync(configPath, "utf-8");
  } catch {
    // File doesn't exist yet
  }

  // Remove any existing webclaw MCP section
  existing = existing.replace(
    /\n?\[mcp_servers\.webclaw\][^\[]*(?=\[|$)/gs,
    "",
  );

  let section = `\n[mcp_servers.webclaw]\ncommand = "${BINARY_PATH}"\nargs = []\nenabled = true\n`;
  if (apiKey) {
    section += `env = { WEBCLAW_API_KEY = "${apiKey}" }\n`;
  }

  writeFileSync(configPath, existing.trimEnd() + "\n" + section);
}

const CONFIG_WRITERS = {
  "claude-desktop": addToClaudeDesktop,
  "claude-code": addToClaudeCode,
  cursor: addToCursor,
  windsurf: addToWindsurf,
  "vscode-continue": addToVSCodeContinue,
  opencode: addToOpenCode,
  antigravity: addToAntigravity,
  codex: addToCodex,
};

// ── Main ──

async function main() {
  console.log();
  console.log(c("bold", "  ┌─────────────────────────────────────┐"));
  console.log(
    c("bold", "  │") +
      c("cyan", "  webclaw") +
      c("dim", " — MCP setup for AI agents") +
      c("bold", "  │"),
  );
  console.log(c("bold", "  └─────────────────────────────────────┘"));
  console.log();

  // 1. Detect installed AI tools
  console.log(c("bold", "  Detecting AI tools..."));
  console.log();

  const detected = AI_TOOLS.filter((tool) => {
    try {
      return tool.detect();
    } catch {
      return false;
    }
  });

  if (detected.length === 0) {
    console.log(c("yellow", "  No supported AI tools detected."));
    console.log();
    console.log(c("dim", "  Supported tools:"));
    for (const tool of AI_TOOLS) {
      console.log(c("dim", `    • ${tool.name}`));
    }
    console.log();
    console.log(
      c("dim", "  Install one of these tools and run this command again."),
    );
    console.log(c("dim", "  Or use --manual to configure manually."));
    console.log();

    if (process.argv.includes("--manual")) {
      // Continue anyway for manual setup
    } else {
      process.exit(0);
    }
  }

  for (const tool of detected) {
    console.log(c("green", `  ✓ ${tool.name}`));
  }
  console.log();

  // 2. Ask for API key
  console.log(c("dim", "  An API key enables cloud features."));
  console.log(
    c("dim", "  Without one, webclaw runs locally (free, no account needed)."),
  );
  console.log();

  const apiKey = await ask(
    c("bold", "  API key ") +
      c("dim", "(press Enter to skip for local-only): "),
  );
  console.log();

  // 3. Download binary
  console.log(c("bold", "  Downloading webclaw-mcp..."));

  const target = getTarget();
  if (!target) {
    console.log(c("red", `  Unsupported platform: ${platform()}-${arch()}`));
    console.log(
      c(
        "dim",
        "  Build from source: cargo install --git https://github.com/0xMassi/webclaw webclaw-mcp",
      ),
    );
    process.exit(1);
  }

  if (!existsSync(INSTALL_DIR)) {
    mkdirSync(INSTALL_DIR, { recursive: true });
  }

  let downloaded = false;
  let prebuiltError = null;

  try {
    // Resolve the latest release. Its tag_name drives the asset name. An
    // unauthenticated GitHub API call is rate-limited to 60/hour per IP, so
    // honour GITHUB_TOKEN when set — but only on this api.github.com request,
    // never on the asset download (which redirects to a CDN).
    const apiHeaders = process.env.GITHUB_TOKEN
      ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` }
      : {};
    const release = JSON.parse(
      (
        await download(
          `https://api.github.com/repos/${REPO}/releases/latest`,
          apiHeaders,
        )
      ).toString(),
    );

    const version = release.tag_name; // e.g. "v0.6.13"
    const ext = IS_WINDOWS ? "zip" : "tar.gz";
    const assetName = `webclaw-${version}-${target}.${ext}`;
    const asset = release.assets?.find((a) => a.name === assetName);
    if (!asset) {
      throw new Error(`asset ${assetName} not found in release ${version}`);
    }

    const archivePath = join(INSTALL_DIR, assetName);
    await downloadFile(asset.browser_download_url, archivePath);

    // Each archive holds a top-level `webclaw-<version>-<target>/` directory
    // containing webclaw, webclaw-mcp, webclaw-server, and docs.
    if (ext === "tar.gz") {
      execSync(`tar xzf "${archivePath}" -C "${INSTALL_DIR}"`, {
        stdio: "ignore",
      });
    } else if (IS_WINDOWS) {
      // Windows ships no `unzip`; Expand-Archive comes with PowerShell 5+.
      execSync(
        `powershell -NoProfile -Command "Expand-Archive -Path '${archivePath}' -DestinationPath '${INSTALL_DIR}' -Force"`,
        { stdio: "ignore" },
      );
    } else {
      execSync(`unzip -o "${archivePath}" -d "${INSTALL_DIR}"`, {
        stdio: "ignore",
      });
    }

    // Lift webclaw-mcp out of the extracted directory to BINARY_PATH, then
    // drop the rest (the other two binaries + docs).
    const extractedDir = join(INSTALL_DIR, `webclaw-${version}-${target}`);
    const extractedBin = join(extractedDir, BINARY_NAME);
    if (!existsSync(extractedBin)) {
      throw new Error(`binary missing after extract: ${extractedBin}`);
    }
    copyFileSync(extractedBin, BINARY_PATH);
    if (!IS_WINDOWS) await chmod(BINARY_PATH, 0o755);

    try {
      rmSync(extractedDir, { recursive: true, force: true });
      rmSync(archivePath, { force: true });
    } catch {}

    console.log(c("green", `  ✓ Installed to ${BINARY_PATH}`));
    downloaded = true;
  } catch (e) {
    prebuiltError = e;
  }

  if (!downloaded) {
    // Surface why the prebuilt path failed instead of hiding it — a 403 here
    // is almost always a GitHub API rate limit, which Rust can't fix.
    if (prebuiltError) {
      const m = prebuiltError.message || String(prebuiltError);
      if (m.includes("403") || /rate limit/i.test(m)) {
        console.log(
          c(
            "yellow",
            "  GitHub API rate limit hit. Retry in a few minutes, or set GITHUB_TOKEN.",
          ),
        );
      } else {
        console.log(c("yellow", `  Prebuilt binary unavailable (${m}).`));
      }
    }

    // Fall back to building from source.
    console.log(c("yellow", "  Trying cargo install..."));
    try {
      execSync(
        `cargo install --git https://github.com/${REPO} webclaw-mcp --root "${INSTALL_DIR}"`,
        { stdio: "inherit" },
      );
      // cargo install puts the binary in INSTALL_DIR/bin/
      const cargoPath = join(INSTALL_DIR, "bin", BINARY_NAME);
      if (existsSync(cargoPath)) {
        copyFileSync(cargoPath, BINARY_PATH);
        console.log(c("green", `  ✓ Built and installed to ${BINARY_PATH}`));
        downloaded = true;
      }
    } catch {
      console.log(
        c("red", "  Failed to install. Make sure Rust is installed:"),
      );
      console.log(
        c(
          "dim",
          "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh",
        ),
      );
      process.exit(1);
    }
  }

  console.log();

  // 4. Configure each detected tool
  console.log(c("bold", "  Configuring MCP servers..."));
  console.log();

  for (const tool of detected) {
    const configPath = tool.configPath();
    if (!configPath) continue;

    const writer = CONFIG_WRITERS[tool.id];
    if (!writer) continue;

    try {
      writer(configPath, apiKey || null);
      console.log(
        c("green", `  ✓ ${tool.name}`) + c("dim", ` → ${configPath}`),
      );
    } catch (e) {
      console.log(c("red", `  ✗ ${tool.name}: ${e.message}`));
    }
  }

  console.log();

  // 5. Verify
  if (downloaded) {
    try {
      const version = execSync(`"${BINARY_PATH}" --version`, {
        encoding: "utf-8",
      }).trim();
      console.log(c("green", `  ✓ ${version}`));
    } catch {
      console.log(c("green", `  ✓ webclaw-mcp installed`));
    }
  }

  // 6. Summary
  console.log();
  console.log(c("bold", "  Done! webclaw is ready."));
  console.log();
  console.log(c("dim", "  Your AI agent now has these tools:"));
  console.log(c("dim", "    • scrape — extract content from any URL"));
  console.log(c("dim", "    • crawl  — recursively crawl a website"));
  console.log(c("dim", "    • search — web search + parallel scrape"));
  console.log(c("dim", "    • map    — discover URLs from sitemaps"));
  console.log(c("dim", "    • batch  — extract multiple URLs in parallel"));
  console.log();

  if (!apiKey) {
    console.log(c("yellow", "  Running in local-only mode (no API key)."));
    console.log(
      c(
        "dim",
        "  Get an API key at https://webclaw.io/dashboard for cloud features.",
      ),
    );
    console.log();
  }

  console.log(c("dim", "  Restart your AI tool to activate the MCP server."));
  console.log();
}

main().catch((e) => {
  console.error(c("red", `\n  Error: ${e.message}\n`));
  process.exit(1);
});
