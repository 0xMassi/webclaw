#!/usr/bin/env node

// create-webclaw — optional convenience installer for the webclaw MCP server.
//
// It auto-detects your AI tools (Claude Desktop, Claude Code, Cursor, Windsurf,
// OpenCode, Codex, Antigravity, ...) and writes the canonical `npx @webclaw/mcp`
// config into each. It does NOT download a binary: the runtime is always the
// `@webclaw/mcp` launcher, so a scaffolded config is byte-identical to what
// webclaw.io/docs and the MCP registries document. If you'd rather not run this,
// just add the one config block below by hand.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from "fs";
import { createInterface } from "readline";
import { homedir, platform } from "os";
import { join, dirname } from "path";

// ── Constants ──

const MCP_PACKAGE = "@webclaw/mcp";

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

// The single canonical way to run the webclaw MCP server: the npx launcher.
// `@webclaw/mcp` fetches the prebuilt binary on first run and speaks MCP over
// stdio, so this config matches webclaw.io/docs and the MCP registries exactly.
function buildMcpEntry(apiKey) {
  const entry = { command: "npx", args: ["-y", MCP_PACKAGE] };
  if (apiKey) entry.env = { WEBCLAW_API_KEY: apiKey };
  return entry;
}

const MANUAL_CONFIG = `{ "mcpServers": { "webclaw": { "command": "npx", "args": ["-y", "${MCP_PACKAGE}"] } } }`;

// ── MCP Config Writers ──

function addToMcpServers(configPath, apiKey) {
  const config = readJsonFile(configPath);
  if (!config.mcpServers) config.mcpServers = {};
  config.mcpServers.webclaw = buildMcpEntry(apiKey);
  writeJsonFile(configPath, config);
}

function addToVSCodeContinue(configPath, apiKey) {
  const config = readJsonFile(configPath);
  if (!config.mcpServers) config.mcpServers = [];
  // Continue uses array format.
  const existing = config.mcpServers.findIndex?.((s) => s.name === "webclaw");
  const entry = { name: "webclaw", ...buildMcpEntry(apiKey) };
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
    command: ["npx", "-y", MCP_PACKAGE],
    enabled: true,
  };
  if (apiKey) {
    config.mcp.webclaw.environment = { WEBCLAW_API_KEY: apiKey };
  }
  writeJsonFile(configPath, config);
}

function addToCodex(configPath, apiKey) {
  // Codex uses TOML, not JSON. Replace any existing webclaw section.
  const dir = dirname(configPath);
  if (!existsSync(dir)) mkdirSync(dir, { recursive: true });

  let existing = "";
  try {
    existing = readFileSync(configPath, "utf-8");
  } catch {
    // File doesn't exist yet.
  }
  existing = existing.replace(
    /\n?\[mcp_servers\.webclaw\][^\[]*(?=\[|$)/gs,
    "",
  );

  let section = `\n[mcp_servers.webclaw]\ncommand = "npx"\nargs = ["-y", "${MCP_PACKAGE}"]\nenabled = true\n`;
  if (apiKey) {
    section += `env = { WEBCLAW_API_KEY = "${apiKey}" }\n`;
  }
  writeFileSync(configPath, existing.trimEnd() + "\n" + section);
}

const CONFIG_WRITERS = {
  "claude-desktop": addToMcpServers,
  "claude-code": addToMcpServers,
  cursor: addToMcpServers,
  windsurf: addToMcpServers,
  "vscode-continue": addToVSCodeContinue,
  opencode: addToOpenCode,
  antigravity: addToMcpServers,
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

  // 1. Detect installed AI tools.
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
    console.log(c("dim", "  Add this to your MCP client config by hand:"));
    console.log(c("cyan", `    ${MANUAL_CONFIG}`));
    console.log();
    if (!process.argv.includes("--manual")) {
      process.exit(0);
    }
  }

  for (const tool of detected) {
    console.log(c("green", `  ✓ ${tool.name}`));
  }
  console.log();

  // 2. Ask for an optional API key.
  console.log(
    c("dim", "  An API key unlocks the cloud tools (bot bypass, JS rendering,"),
  );
  console.log(
    c("dim", "  search, research, leads). Without one, webclaw runs locally."),
  );
  console.log();

  const apiKey = await ask(
    c("bold", "  API key ") +
      c("dim", "(press Enter to skip for local-only): "),
  );
  console.log();

  // 3. Write the npx config into each detected tool.
  console.log(c("bold", "  Writing MCP config..."));
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

  // 4. Summary.
  console.log(c("bold", "  Done! webclaw is configured."));
  console.log();
  console.log(
    c("dim", "  Your client runs the server on demand via npx — nothing to"),
  );
  console.log(
    c("dim", "  install. First launch fetches @webclaw/mcp, then it's cached."),
  );
  console.log();
  console.log(
    c("dim", "  Tools: scrape, search, crawl, map, batch, extract, summarize,"),
  );
  console.log(
    c(
      "dim",
      "  diff, brand, research, lead, lead_batch, + 30 site extractors.",
    ),
  );
  console.log();

  if (!apiKey) {
    console.log(c("yellow", "  Running in local-only mode (no API key)."));
    console.log(
      c(
        "dim",
        "  Get a key at https://webclaw.io/dashboard for cloud features.",
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
