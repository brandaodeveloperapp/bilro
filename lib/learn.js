import { createHash } from "node:crypto";
import { DatabaseSync } from "node:sqlite";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

const HOME = join(homedir(), ".claude", "bilro");

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

export function lineHash(line) {
  return createHash("sha1")
    .update(line.trim().replace(/[0-9a-f]{7,}/g, "H").replace(/\d+(?:[.,]\d+)?/g, "N"))
    .digest("hex")
    .slice(0, 16);
}

export function open(file = join(HOME, "learn.db")) {
  mkdirSync(HOME, { recursive: true });
  const db = new DatabaseSync(file);
  db.exec(`
    CREATE TABLE IF NOT EXISTS runs (sig TEXT PRIMARY KEY, n INTEGER NOT NULL, last_hash TEXT);
    CREATE TABLE IF NOT EXISTS lines (sig TEXT, h TEXT, df INTEGER NOT NULL, PRIMARY KEY (sig, h));
  `);
  return db;
}

export function runCount(db, sig) {
  const row = db.prepare("SELECT n, last_hash FROM runs WHERE sig = ?").get(sig);
  return row || { n: 0, last_hash: null };
}

/**
 * Records one run: how many times this command shape has been seen, and how
 * many of those runs each line appeared in.
 */
export function observe(db, command, output) {
  const sig = signature(command);
  const hash = createHash("sha1").update(output).digest("hex");
  const uniq = new Set(output.split("\n").filter((l) => l.trim()).map(lineHash));

  const prior = runCount(db, sig);
  db.prepare("INSERT INTO runs(sig, n, last_hash) VALUES (?, 1, ?) ON CONFLICT(sig) DO UPDATE SET n = n + 1, last_hash = ?").run(
    sig,
    hash,
    hash,
  );
  const bump = db.prepare(
    "INSERT INTO lines(sig, h, df) VALUES (?, ?, 1) ON CONFLICT(sig, h) DO UPDATE SET df = df + 1",
  );
  for (const h of uniq) bump.run(sig, h);

  return { sig, hash, runs: prior.n + 1, unchanged: prior.last_hash === hash && prior.n > 0 };
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
  const kept = output.split("\n").filter((line) => {
    if (!line.trim()) return true;
    const seen = df.get(lineHash(line)) || 0;
    if (seen / n > ceiling) {
      dropped++;
      return false;
    }
    return true;
  });

  return { text: kept.join("\n").trim(), learned: true, runs: n, dropped };
}

/** Lines present now that were not in the previous run of the same command. */
export function novelty(previous, current) {
  const before = new Set(previous.split("\n").map(lineHash));
  return current.split("\n").filter((l) => l.trim() && !before.has(lineHash(l)));
}
