#!/usr/bin/env node
/**
 * @webclaw/mcp — zero-install launcher for the webclaw MCP server.
 *
 * `npx -y @webclaw/mcp` resolves a prebuilt `webclaw-mcp` binary (downloaded
 * once from the pinned GitHub release, then cached) and execs it as an MCP
 * stdio server. This is what makes webclaw introspectable/installable in MCP
 * clients and registries (Claude Desktop, Cursor, Glama, Smithery, ...) without
 * a Rust build.
 *
 * HARD RULE: stdout carries the MCP JSON-RPC stream. Every diagnostic this
 * launcher emits MUST go to stderr, or it corrupts the protocol. Never
 * console.log here — only logErr().
 *
 * Overrides (env):
 *   WEBCLAW_MCP_BIN      absolute path to a webclaw-mcp binary; skip download
 *   WEBCLAW_MCP_VERSION  release tag to install (default: pinned RELEASE_TAG)
 *   WEBCLAW_MCP_CACHE    cache root (default: ~/.cache/webclaw)
 *   GITHUB_TOKEN         only used to fetch SHA256SUMS if rate-limited
 */

import {
  existsSync,
  mkdirSync,
  createWriteStream,
  readFileSync,
  renameSync,
  copyFileSync,
  rmSync,
  chmodSync,
} from "node:fs";
import { homedir, platform, arch } from "node:os";
import { join } from "node:path";
import { spawn, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import https from "node:https";

const REPO = "0xMassi/webclaw";
// Release the wrapper installs. Bump this (and the package version) on each
// core release, or override at runtime with WEBCLAW_MCP_VERSION.
const RELEASE_TAG = process.env.WEBCLAW_MCP_VERSION || "v0.6.15";

const IS_WINDOWS = platform() === "win32";
const BIN_NAME = IS_WINDOWS ? "webclaw-mcp.exe" : "webclaw-mcp";
const CACHE_ROOT =
  process.env.WEBCLAW_MCP_CACHE || join(homedir(), ".cache", "webclaw");
const CACHE_DIR = join(CACHE_ROOT, RELEASE_TAG);
const CACHED_BIN = join(CACHE_DIR, BIN_NAME);

function logErr(msg) {
  process.stderr.write(`[@webclaw/mcp] ${msg}\n`);
}

function target() {
  const map = {
    "darwin-arm64": "aarch64-apple-darwin",
    "darwin-x64": "x86_64-apple-darwin",
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "win32-x64": "x86_64-pc-windows-msvc",
  };
  return map[`${platform()}-${arch()}`] || null;
}

// GET a URL to a Buffer, following redirects. Auth headers are dropped on
// redirect so a token never reaches the release CDN (its signed URLs reject it).
function getBuffer(url, headers = {}) {
  return new Promise((resolve, reject) => {
    https
      .get(
        url,
        { headers: { "User-Agent": "@webclaw/mcp", ...headers } },
        (res) => {
          if (
            res.statusCode >= 300 &&
            res.statusCode < 400 &&
            res.headers.location
          ) {
            res.resume();
            return getBuffer(res.headers.location).then(resolve, reject);
          }
          if (res.statusCode !== 200) {
            res.resume();
            return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
          }
          const chunks = [];
          res.on("data", (c) => chunks.push(c));
          res.on("end", () => resolve(Buffer.concat(chunks)));
          res.on("error", reject);
        },
      )
      .on("error", reject);
  });
}

// Stream a URL to a file, following redirects.
function getFile(url, dest) {
  return new Promise((resolve, reject) => {
    https
      .get(url, { headers: { "User-Agent": "@webclaw/mcp" } }, (res) => {
        if (
          res.statusCode >= 300 &&
          res.statusCode < 400 &&
          res.headers.location
        ) {
          res.resume();
          return getFile(res.headers.location, dest).then(resolve, reject);
        }
        if (res.statusCode !== 200) {
          res.resume();
          return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        }
        const out = createWriteStream(dest);
        res.pipe(out);
        out.on("finish", () => out.close(() => resolve()));
        out.on("error", reject);
      })
      .on("error", reject);
  });
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function extract(archivePath, destDir) {
  if (IS_WINDOWS) {
    execFileSync(
      "powershell",
      [
        "-NoProfile",
        "-Command",
        `Expand-Archive -Path '${archivePath}' -DestinationPath '${destDir}' -Force`,
      ],
      { stdio: "ignore" },
    );
  } else {
    execFileSync("tar", ["xzf", archivePath, "-C", destDir], {
      stdio: "ignore",
    });
  }
}

async function ensureBinary() {
  // 0. Explicit local binary (dev / offline / CI).
  const override = process.env.WEBCLAW_MCP_BIN;
  if (override) {
    if (!existsSync(override)) {
      throw new Error(`WEBCLAW_MCP_BIN points at a missing file: ${override}`);
    }
    return override;
  }

  // 1. Cache hit for this release.
  if (existsSync(CACHED_BIN)) return CACHED_BIN;

  // 2. Download, verify, extract, cache.
  const tgt = target();
  if (!tgt) {
    throw new Error(
      `unsupported platform ${platform()}-${arch()} — install the webclaw-mcp binary manually ` +
        `(https://github.com/${REPO}/releases) and set WEBCLAW_MCP_BIN`,
    );
  }
  const ext = IS_WINDOWS ? "zip" : "tar.gz";
  const assetName = `webclaw-${RELEASE_TAG}-${tgt}.${ext}`;
  const base = `https://github.com/${REPO}/releases/download/${RELEASE_TAG}`;

  mkdirSync(CACHE_DIR, { recursive: true });
  const archivePath = join(CACHE_DIR, assetName);
  const tmpPath = `${archivePath}.download`;

  logErr(`fetching ${assetName} (first run for ${RELEASE_TAG}) ...`);
  await getFile(`${base}/${assetName}`, tmpPath);

  // Verify against the release SHA256SUMS. If the sums file can't be fetched we
  // proceed with a warning rather than hard-failing an otherwise-good install.
  try {
    const apiHeaders = process.env.GITHUB_TOKEN
      ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` }
      : {};
    const sums = (await getBuffer(`${base}/SHA256SUMS`, apiHeaders)).toString(
      "utf8",
    );
    const line = sums.split("\n").find((l) => l.trim().endsWith(assetName));
    const expected = line ? line.trim().split(/\s+/)[0] : null;
    if (expected) {
      const actual = sha256(tmpPath);
      if (actual !== expected) {
        rmSync(tmpPath, { force: true });
        throw new Error(
          `checksum mismatch for ${assetName}: expected ${expected}, got ${actual}`,
        );
      }
    } else {
      logErr(
        `warning: ${assetName} not found in SHA256SUMS — skipping verification`,
      );
    }
  } catch (e) {
    if (/checksum mismatch/.test(e.message)) throw e;
    logErr(`warning: could not verify checksum (${e.message}) — proceeding`);
  }

  renameSync(tmpPath, archivePath);
  extract(archivePath, CACHE_DIR);

  // Archives hold a top-level `webclaw-<tag>-<target>/` dir with all binaries.
  const extractedDir = join(CACHE_DIR, `webclaw-${RELEASE_TAG}-${tgt}`);
  const extractedBin = join(extractedDir, BIN_NAME);
  if (!existsSync(extractedBin)) {
    throw new Error(`binary missing after extract: ${extractedBin}`);
  }
  copyFileSync(extractedBin, CACHED_BIN);
  if (!IS_WINDOWS) chmodSync(CACHED_BIN, 0o755);

  // Drop the archive and the extra binaries; keep only the cached webclaw-mcp.
  try {
    rmSync(extractedDir, { recursive: true, force: true });
    rmSync(archivePath, { force: true });
  } catch {
    /* non-fatal */
  }

  logErr(`installed ${BIN_NAME} for ${RELEASE_TAG} → ${CACHED_BIN}`);
  return CACHED_BIN;
}

async function main() {
  let bin;
  try {
    bin = await ensureBinary();
  } catch (e) {
    logErr(`error: ${e.message}`);
    process.exit(1);
  }

  // Hand off to the real server. stdio:inherit wires the MCP JSON-RPC stream
  // straight through; env passes WEBCLAW_API_KEY and friends unchanged.
  const child = spawn(bin, process.argv.slice(2), {
    stdio: "inherit",
    env: process.env,
  });

  child.on("error", (e) => {
    logErr(`failed to start webclaw-mcp: ${e.message}`);
    process.exit(1);
  });
  child.on("exit", (code, signal) => {
    if (signal) {
      // Re-raise the signal so the parent's exit reflects it.
      process.kill(process.pid, signal);
    } else {
      process.exit(code ?? 0);
    }
  });

  // Forward termination signals to the child so clients can stop the server.
  for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) {
    process.on(sig, () => {
      if (!child.killed) child.kill(sig);
    });
  }
}

main();
