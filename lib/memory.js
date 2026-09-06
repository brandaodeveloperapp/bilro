import { readFileSync, readdirSync } from "node:fs";
import { join, basename } from "node:path";

const DAY = 24 * 60 * 60 * 1000;

/** Claims that go stale silently: versions, ids, counts, money, dates. */
const CLAIM_PATTERNS = [
  { kind: "versao", re: /\bv?\d+\.\d+\.\d+\b/g },
  { kind: "build", re: /\b(?:versionCode|build(?:Number)?)\s*:?\s*\d+/gi },
  { kind: "valor", re: /R\$\s?[\d.,]+/g },
  { kind: "contagem", re: /\b\d{2,}\s+(?:tenants?|usuarios?|referrals?|comiss[oõ]es|pods?)\b/gi },
];

export function parseMemory(path) {
  const text = readFileSync(path, "utf8");
  const fm = text.match(/^---\n([\s\S]*?)\n---/);
  const head = fm ? fm[1] : "";
  const field = (name) => {
    const m = head.match(new RegExp(`^\\s*${name}:\\s*(.+)$`, "m"));
    return m ? m[1].trim().replace(/^["']|["']$/g, "") : null;
  };
  return {
    path,
    name: field("name") || basename(path, ".md"),
    type: field("type"),
    modified: field("modified"),
    verify: field("verify"),
    expect: field("expect"),
    body: fm ? text.slice(fm[0].length) : text,
  };
}

export function loadMemories(dir) {
  let files = [];
  try {
    files = readdirSync(dir).filter((f) => f.endsWith(".md") && f !== "MEMORY.md" && !f.includes(".bak-"));
  } catch {
    return [];
  }
  return files.map((f) => parseMemory(join(dir, f)));
}

export function claimsIn(memory) {
  const found = [];
  for (const { kind, re } of CLAIM_PATTERNS) {
    const hits = memory.body.match(re);
    if (hits) found.push({ kind, samples: [...new Set(hits)].slice(0, 4) });
  }
  return found;
}

export function ageInDays(memory, now = Date.now()) {
  if (!memory.modified) return null;
  const t = Date.parse(memory.modified);
  return Number.isNaN(t) ? null : Math.floor((now - t) / DAY);
}

/**
 * A memory is suspect when it asserts a fact that drifts, declares no way to
 * check itself, and has not been touched in a while.
 */
export function audit(memories, { staleAfterDays = 21, now = Date.now() } = {}) {
  const out = [];
  for (const m of memories) {
    const claims = claimsIn(m);
    const age = ageInDays(m, now);
    if (m.verify) {
      out.push({ memory: m, status: "verificavel", claims, age });
    } else if (claims.length && age !== null && age >= staleAfterDays) {
      out.push({ memory: m, status: "suspeita", claims, age });
    } else if (claims.length) {
      out.push({ memory: m, status: "afirma", claims, age });
    }
  }
  return out;
}
