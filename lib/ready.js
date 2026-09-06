import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";
import { open, denoise, isSevere } from "./learn.js";

const HOME = join(homedir(), ".claude", "bilro");
const AUDITS = join(HOME, "audits.json");

const GATE = { coverage: 0.8, minAudits: 2, maxCriticals: 0, savings: 0.5, minCommands: 40 };

export function audits() {
  try {
    return JSON.parse(readFileSync(AUDITS, "utf8"));
  } catch {
    return [];
  }
}

export function recordAudit(entry) {
  const all = audits();
  all.push({ at: Date.now(), ...entry });
  mkdirSync(HOME, { recursive: true });
  writeFileSync(AUDITS, JSON.stringify(all, null, 2));
  return all;
}

/**
 * Replays every command bilro has learned and measures what it would do to
 * that output today. Coverage says how much of the real workload it can even
 * act on; the signal check is the invariant that matters most — a saver that
 * eats a failure line is worse than no saver at all.
 */
export function evaluate(db = open()) {
  const rows = db.prepare("SELECT r.sig, r.n, l.body FROM runs r LEFT JOIN last l ON l.sig = r.sig").all();
  const total = rows.length;
  let learned = 0;
  let rawBytes = 0;
  let keptBytes = 0;
  const lost = [];

  for (const row of rows) {
    if (!row.body) continue;
    if (row.n >= 3) learned++;
    const r = denoise(db, row.sig, row.body);
    rawBytes += row.body.length;
    keptBytes += r.text.length;
    const severeIn = row.body.split("\n").filter(isSevere);
    if (severeIn.length) {
      const out = r.text;
      for (const line of severeIn) if (!out.includes(line.trim())) lost.push({ sig: row.sig, line: line.trim().slice(0, 80) });
    }
  }

  const a = audits();
  const criticals = a.reduce((n, x) => n + (x.critical || 0), 0);
  return {
    total,
    learned,
    coverage: total ? learned / total : 0,
    savings: rawBytes ? 1 - keptBytes / rawBytes : 0,
    lost,
    audits: a.length,
    criticals,
    lastAudit: a.length ? a[a.length - 1].at : null,
  };
}

/** One verdict per tool, ordered by how much damage a wrong swap would do. */
export function verdicts(m) {
  const enough = m.total >= GATE.minCommands;
  const safe = m.lost.length === 0;
  const audited = m.audits >= GATE.minAudits && m.criticals <= GATE.maxCriticals;
  const covered = m.coverage >= GATE.coverage;
  const saving = m.savings >= GATE.savings;

  const why = (...pairs) => pairs.filter(([, ok]) => !ok).map(([name]) => name);
  return [
    { tool: "caveman", risk: "texto feio", missing: why(["auditoria", audited]) },
    { tool: "context-mode", risk: "busca pior", missing: why(["auditoria", audited], ["historico", enough]) },
    {
      tool: "rtk",
      risk: "output comido em silencio",
      missing: why(["auditoria", audited], ["historico", enough], ["cobertura", covered], ["economia", saving], ["sinal", safe]),
    },
  ];
}

export { GATE };
