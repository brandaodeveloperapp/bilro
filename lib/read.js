import { readFileSync } from "node:fs";

const DECL =
  /^\s*(export\s+)?(async\s+)?(function|class|const|let|var|interface|type|enum|def|struct|impl|trait|fn|public|private|protected|static|module|namespace|describe|it|test)\b|^\s*[\w$.]+\s*[:=]\s*(async\s*)?(\([^)]*\)|function)\s*(=>)?\s*\{?\s*$|^\s*(@|#\[)/;

const BLANK = /^\s*$/;

/**
 * Whitespace-only compression. Meaning is untouched, so the result stays safe
 * to reason about byte for byte; the saving is small but costs nothing.
 */
export function safe(text) {
  const out = [];
  let blanks = 0;
  for (const line of text.split("\n")) {
    const trimmed = line.replace(/\s+$/, "");
    if (BLANK.test(trimmed)) {
      if (++blanks > 1) continue;
    } else blanks = 0;
    out.push(trimmed);
  }
  return out.join("\n");
}

/**
 * Structure without bodies: declarations survive, the code between them is
 * replaced by a marker naming the exact line range, so anything elided can be
 * fetched precisely instead of guessed at.
 */
export function outline(text) {
  const lines = text.split("\n");
  const keep = new Set();
  for (let i = 0; i < lines.length; i++) {
    if (DECL.test(lines[i])) {
      keep.add(i);
      if (i > 0 && /^\s*(\/\*\*|\*|\/\/|#)/.test(lines[i - 1])) keep.add(i - 1);
    }
  }
  const out = [];
  let gap = null;
  const flush = () => {
    if (!gap) return;
    const n = gap.end - gap.start + 1;
    if (n <= 2) for (let i = gap.start; i <= gap.end; i++) out.push(lines[i]);
    else out.push(`      … ${n} linhas (${gap.start + 1}-${gap.end + 1})`);
    gap = null;
  };
  for (let i = 0; i < lines.length; i++) {
    if (keep.has(i)) {
      flush();
      out.push(lines[i]);
    } else if (gap) gap.end = i;
    else gap = { start: i, end: i };
  }
  flush();
  return out.join("\n");
}

export function read(file, { mode = "safe" } = {}) {
  const raw = readFileSync(file, "utf8");
  const text = mode === "outline" ? outline(raw) : safe(raw);
  return {
    text,
    mode,
    lossy: mode === "outline",
    before: raw.length,
    after: text.length,
    saved: raw.length ? 1 - text.length / raw.length : 0,
  };
}
