import { createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

const HOME = join(homedir(), ".claude", "bilro");

const MAX_LINES = 20000;

/**
 * A line that reports a failure is never noise, however often it repeats. A
 * build that breaks the same way every day is still the answer to "what
 * happened", and the whole point of the tool is lost if it hides it.
 */
const SEVERE = /\b(error|erro|failed|failing|failure|falhou|fatal|panic|exception|traceback|refused|denied|unauthorized|forbidden|timeout|timed out|cannot|could not|no such|not found|undefined is not|segmentation fault|✕|✗|FAIL)\b/i;

export function isSevere(line) {
  return SEVERE.test(line);
}

/** Commands differ by their arguments; what repeats is the program and shape. */
export function signature(command) {
  return command
    .replace(/["'][^"']*["']/g, "S")
    .replace(/\b[0-9a-f]{7,40}\b/g, "H")
    .replace(/\b\d+\b/g, "N")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 200);
}

function digest(text) {
  return createHash("sha1").update(text).digest("hex").slice(0, 16);
}

/**
 * Shape identity: numbers and hashes collapse, so a duration or a counter does
 * not make every run look new. Used to decide what is noise.
 */
export function lineHash(line) {
  return digest(line.trim().replace(/[0-9a-f]{7,}/g, "H").replace(/\d+(?:[.,]\d+)?/g, "N"));
}

/**
 * Exact identity, numbers included. Used to decide what is new — "modulo 3
 * falhou" and "modulo 7 falhou" are different events even though they share a
 * shape.
 */
export function exactHash(line) {
  return digest(line.trim());
}

export function open(file = join(HOME, "learn.db")) {
  mkdirSync(HOME, { recursive: true });
  const db = new DatabaseSync(file);
  db.exec("PRAGMA busy_timeout = 5000; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;");
  db.exec(`
    CREATE TABLE IF NOT EXISTS runs (sig TEXT PRIMARY KEY, n INTEGER NOT NULL, last_hash TEXT);
    CREATE TABLE IF NOT EXISTS lines (sig TEXT, h TEXT, df INTEGER NOT NULL, PRIMARY KEY (sig, h));
    CREATE TABLE IF NOT EXISTS last (sig TEXT PRIMARY KEY, body TEXT);
    CREATE TABLE IF NOT EXISTS exact (sig TEXT, shape TEXT, h TEXT, PRIMARY KEY (sig, h));
  `);
  return db;
}

export function runCount(db, sig) {
  const row = db.prepare("SELECT n, last_hash FROM runs WHERE sig = ?").get(sig);
  if (!row) return { n: 0, last_hash: null, body: null };
  const prev = db.prepare("SELECT body FROM last WHERE sig = ?").get(sig);
  return { ...row, body: prev ? prev.body : null };
}

/**
 * Records one run: how many times this command shape has been seen, and how
 * many of those runs each line appeared in.
 */
export function observe(db, command, output) {
  const sig = signature(command);
  const hash = createHash("sha1").update(output).digest("hex");
  const all = output.split("\n");
  const lines = all.length > MAX_LINES ? all.slice(0, MAX_LINES) : all;
  const uniq = new Set(lines.filter((l) => l.trim()).map(lineHash));

  const prior = runCount(db, sig);
  db.exec("BEGIN IMMEDIATE");
  try {
  db.prepare("INSERT INTO runs(sig, n, last_hash) VALUES (?, 1, ?) ON CONFLICT(sig) DO UPDATE SET n = n + 1, last_hash = ?").run(
    sig,
    hash,
    hash,
  );
  const bump = db.prepare(
    "INSERT INTO lines(sig, h, df) VALUES (?, ?, 1) ON CONFLICT(sig, h) DO UPDATE SET df = df + 1",
  );
  for (const h of uniq) bump.run(sig, h);
  const seenExact = db.prepare("INSERT OR IGNORE INTO exact(sig, shape, h) VALUES (?, ?, ?)");
  for (const line of lines) if (line.trim()) seenExact.run(sig, lineHash(line), exactHash(line));

  db.prepare("INSERT INTO last(sig, body) VALUES (?, ?) ON CONFLICT(sig) DO UPDATE SET body = ?").run(sig, output, output);
    db.exec("COMMIT");
  } catch (e) {
    try { db.exec("ROLLBACK"); } catch {}
    throw e;
  }
  return {
    sig,
    hash,
    runs: prior.n + 1,
    unchanged: prior.last_hash === hash && prior.n > 0,
    previous: prior.body || null,
  };
}

/**
 * Drops lines that carry no information: those seen in most previous runs of
 * the same command. Below `minRuns` there is not enough history to judge.
 */
export function denoise(db, command, output, { minRuns = 3, ceiling = 0.8 } = {}) {
  const sig = signature(command);
  const { n } = runCount(db, sig);
  if (n < minRuns) return { text: output, learned: false, runs: n, dropped: 0 };

  const df = new Map();
  for (const row of db.prepare("SELECT h, df FROM lines WHERE sig = ?").all(sig)) df.set(row.h, row.df);

  let dropped = 0;
  const exact = new Set(db.prepare("SELECT h FROM exact WHERE sig = ?").all(sig).map((r) => r.h));
  const spread = new Map();
  for (const row of db.prepare("SELECT shape, count(*) c FROM exact WHERE sig = ? GROUP BY shape").all(sig))
    spread.set(row.shape, row.c);

  const kept = output.split("\n").filter((line) => {
    if (!line.trim()) return true;
    if (SEVERE.test(line)) return true;
    const shape = lineHash(line);
    const seen = df.get(shape) || 0;
    const distinct = spread.get(shape) || 0;
    const volatile_ = distinct / Math.max(n, 1) > 0.5;
    if (!volatile_ && !exact.has(exactHash(line))) return true;
    if (seen / n > ceiling) {
      dropped++;
      return false;
    }
    return true;
  });

  return { text: kept.join("\n").trim(), learned: true, runs: n, dropped };
}

/**
 * Jaccard overlap between two outputs, on normalised line identity. Exact set
 * intersection is cheap at the size of a command's output; MinHash only earns
 * its estimation error when the corpus is far larger than this.
 */
export function similarity(previous, current) {
  const a = new Set(previous.split("\n").filter((l) => l.trim()).map(lineHash));
  const b = new Set(current.split("\n").filter((l) => l.trim()).map(lineHash));
  if (!a.size && !b.size) return 1;
  let shared = 0;
  for (const h of b) if (a.has(h)) shared++;
  return shared / (a.size + b.size - shared);
}

/**
 * When output overruns the budget, keeps the lines that carry the most
 * information rather than an arbitrary head and tail. Order is preserved.
 */
export function rankByInformation(db, command, output, keep = 80) {
  const lines = output.split("\n");
  if (lines.length <= keep) return { text: output, ranked: false, kept: lines.length };
  const sig = signature(command);
  const { n } = runCount(db, sig);
  const df = new Map();
  for (const row of db.prepare("SELECT h, df FROM lines WHERE sig = ?").all(sig)) df.set(row.h, row.df);

  const scored = lines.map((line, i) => {
    const seen = df.get(lineHash(line)) || 0;
    const idf = Math.log((n + 1) / (seen + 1)) + (SEVERE.test(line) ? 10 : 0);
    return { i, line, idf };
  });
  const chosen = new Set(
    scored
      .filter((s) => s.line.trim())
      .sort((a, b) => b.idf - a.idf)
      .slice(0, keep)
      .map((s) => s.i),
  );
  const text = scored
    .filter((s) => chosen.has(s.i))
    .map((s) => s.line)
    .join("\n");
  return { text, ranked: true, kept: chosen.size, from: lines.length };
}

/** Lines present now that were not in the previous run of the same command. */
export function novelty(previous, current) {
  const before = new Set(previous.split("\n").map(exactHash));
  return current.split("\n").filter((l) => l.trim() && !before.has(exactHash(l)));
}
