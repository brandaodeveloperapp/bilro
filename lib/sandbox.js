import { DatabaseSync } from "node:sqlite";
import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

import { tokensOf } from "./weigh.js";

const HOME = join(homedir(), ".claude", "bilro");

function open(file = join(HOME, "index.db")) {
  mkdirSync(HOME, { recursive: true });
  const db = new DatabaseSync(file);
  db.exec("PRAGMA busy_timeout = 5000;");
  db.exec(`
    CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(label, body, source, at UNINDEXED);
  `);
  return db;
}

/** Splits on blank lines so a section stays whole, then caps runaway blocks. */
export function chunk(text, maxLines = 60) {
  const blocks = text.split(/\n{2,}/);
  const out = [];
  for (const block of blocks) {
    const lines = block.split("\n");
    for (let i = 0; i < lines.length; i += maxLines) {
      const piece = lines.slice(i, i + maxLines).join("\n").trim();
      if (piece) out.push(piece);
    }
  }
  return out;
}

export function index(db, { label, body, source }) {
  const stmt = db.prepare("INSERT INTO chunks(label, body, source, at) VALUES (?, ?, ?, ?)");
  const pieces = chunk(body);
  for (const piece of pieces) stmt.run(label, piece, source, Date.now());
  return pieces.length;
}

/** FTS5 reads punctuation as syntax, so every term travels as a quoted phrase. */
export function toMatchQuery(query) {
  const terms = String(query)
    .split(/\s+/)
    .map((t) => t.replace(/["]/g, ""))
    .filter(Boolean);
  if (!terms.length) return null;
  return terms.map((t) => `"${t}"`).join(" OR ");
}

export function search(db, query, limit = 5) {
  const match = toMatchQuery(query);
  if (!match) return [];
  return db
    .prepare(
      `SELECT label, body, source, bm25(chunks) AS score
       FROM chunks WHERE chunks MATCH ? ORDER BY score LIMIT ?`,
    )
    .all(match, limit);
}

/**
 * Runs a command, keeps its output in the index, and hands back only the size
 * of what was withheld plus whatever the queries asked for.
 */
export function run({ command, label, queries = [], cwd = process.cwd() }) {
  let output = "";
  let failed = false;
  try {
    output = execFileSync("/bin/sh", ["-c", command], {
      cwd,
      encoding: "utf8",
      timeout: 120000,
      maxBuffer: 64 * 1024 * 1024,
    });
  } catch (e) {
    failed = true;
    output = String(e.stdout || "") + String(e.stderr || e.message || "");
  }

  const db = open();
  const pieces = index(db, { label: label || command.slice(0, 60), body: output, source: command });
  const hits = queries.flatMap((q) => search(db, q, 3).map((r) => ({ q, ...r })));
  db.close();

  return {
    failed,
    withheldTokens: tokensOf(output),
    chunks: pieces,
    hits,
  };
}

export { open };
