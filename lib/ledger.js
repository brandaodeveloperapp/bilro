import { readFileSync, writeFileSync, renameSync, mkdirSync, readdirSync, statSync, unlinkSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

const DIR = join(homedir(), ".claude", "bilro", "sessions");
const DAY = 24 * 60 * 60 * 1000;

function file(sessionId) {
  return join(DIR, `${sessionId || "unknown"}.json`);
}

export function read(sessionId) {
  try {
    const raw = JSON.parse(readFileSync(file(sessionId), "utf8"));
    return { dispatches: [], withheld: [], filtered: [], ...raw };
  } catch {
    return { dispatches: [], withheld: [], filtered: [] };
  }
}

export function write(sessionId, state) {
  try {
    mkdirSync(DIR, { recursive: true });
    const tmp = `${file(sessionId)}.${process.pid}.tmp`;
    writeFileSync(tmp, JSON.stringify(state));
    renameSync(tmp, file(sessionId));
  } catch {}
}

export function record(sessionId, kind, entry) {
  const state = read(sessionId);
  (state[kind] ||= []).push({ ...entry, at: Date.now() });
  write(sessionId, state);
  return state;
}

export function sessions() {
  try {
    return readdirSync(DIR)
      .filter((f) => f.endsWith(".json"))
      .map((f) => ({ id: f.replace(/\.json$/, ""), mtime: statSync(join(DIR, f)).mtimeMs }))
      .sort((a, b) => b.mtime - a.mtime);
  } catch {
    return [];
  }
}

/** What a session spent on fixed costs, and what it avoided spending. */
export function summarize(sessionId) {
  const s = read(sessionId);
  const dispatchCost = s.dispatches.reduce((n, d) => n + (d.cost || 0), 0);
  const withheld = s.withheld.reduce((n, w) => n + (w.tokens || 0), 0);
  const filtered = s.filtered.reduce((n, f) => n + (f.saved || 0), 0);
  const repeats = s.dispatches.filter((d) => d.repeat).length;
  return {
    dispatches: s.dispatches.length,
    dispatchCost,
    repeats,
    withheld,
    filtered,
    saved: withheld + filtered,
  };
}

export function prune(olderThanDays = 14) {
  let removed = 0;
  const cutoff = Date.now() - olderThanDays * DAY;
  for (const s of sessions()) {
    if (s.mtime < cutoff) {
      try {
        unlinkSync(file(s.id));
        removed++;
      } catch {}
    }
  }
  return removed;
}
