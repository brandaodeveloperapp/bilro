import { readFileSync, readdirSync, statSync, existsSync } from "node:fs";
import { join, basename } from "node:path";
import { homedir } from "node:os";

export const CHARS_PER_TOKEN = 4;

export function tokensOf(text) {
  return Math.round(text.length / CHARS_PER_TOKEN);
}

function readOr(path, fallback = "") {
  try {
    return readFileSync(path, "utf8");
  } catch {
    return fallback;
  }
}

function listFiles(dir, ext = ".md") {
  try {
    return readdirSync(dir)
      .filter((f) => f.endsWith(ext))
      .map((f) => join(dir, f));
  } catch {
    return [];
  }
}

/** Files injected into the system prompt of every request. */
export function weighAlwaysOn(root = join(homedir(), ".claude")) {
  const items = [];
  for (const name of ["CLAUDE.md", "RTK.md"]) {
    const path = join(root, name);
    if (existsSync(path)) items.push({ name, tokens: tokensOf(readOr(path)), path });
  }
  return items;
}

/** Per-project memory index, injected on every request in that project. */
export function weighMemory(root = join(homedir(), ".claude", "projects")) {
  const out = [];
  let projects = [];
  try {
    projects = readdirSync(root);
  } catch {
    return out;
  }
  for (const p of projects) {
    const index = join(root, p, "memory", "MEMORY.md");
    if (!existsSync(index)) continue;
    const files = listFiles(join(root, p, "memory")).filter(
      (f) => basename(f) !== "MEMORY.md" && !basename(f).includes(".bak-"),
    );
    out.push({
      project: p,
      tokens: tokensOf(readOr(index)),
      entries: (readOr(index).match(/^- \[/gm) || []).length,
      files: files.length,
      path: index,
    });
  }
  return out;
}

function frontmatterTools(text) {
  const m = text.match(/^tools:\s*(.+)$/m);
  return m ? m[1].trim() : null;
}

/**
 * Agent definitions: name+description ride in every request, and an agent with
 * no explicit `tools:` inherits the entire tool catalogue at dispatch.
 */
export function weighAgents(dirs) {
  const seen = new Map();
  for (const dir of dirs) {
    for (const path of listFiles(dir)) {
      const text = readOr(path);
      const name = basename(path, ".md");
      const tools = frontmatterTools(text);
      const desc = (text.match(/^description:\s*(.+)$/m) || [, ""])[1];
      seen.set(`${dir}:${name}`, {
        name,
        dir,
        catalogueTokens: tokensOf(`${name} ${desc}`),
        inheritsEverything: tools === null,
        hasContextMode: /ctx_batch_execute|ctx_execute|ctx_search/.test(text),
        path,
      });
    }
  }
  return [...seen.values()];
}

/** MCP servers declared for the CLI — every tool schema loads per request. */
export function weighMcp() {
  const out = [];
  for (const file of [join(homedir(), ".claude.json"), join(homedir(), ".claude", "settings.json")]) {
    try {
      const cfg = JSON.parse(readOr(file, "{}"));
      for (const name of Object.keys(cfg.mcpServers || {})) out.push({ name, source: basename(file) });
    } catch {}
  }
  return out;
}

export function weighPlugins(root = join(homedir(), ".claude", "plugins")) {
  try {
    const cfg = JSON.parse(readOr(join(root, "installed_plugins.json"), "{}"));
    return Object.keys(cfg.plugins || {});
  } catch {
    return [];
  }
}

export function diskUsage(path) {
  try {
    return statSync(path).size;
  } catch {
    return 0;
  }
}
