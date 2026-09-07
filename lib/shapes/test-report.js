/**
 * Shape of a test runner's own output: jest, vitest, pytest, phpunit, pest,
 * rspec, playwright, go test, paratest. Structurally these all agree on the
 * same three things — a flood of per-case pass markers, a short summary count
 * at the end, and (when something breaks) a small number of failure blocks
 * carrying the real detail. Compression keeps the summary and every failure
 * block whole, and drops the flood.
 */
import { isSevere } from "../learn.js";

export const name = "test-report";

const CASE_MARK = /^\s*(PASS|FAIL)\s+\S|^\s*[✓✔✕✗×]\s|^\s*(ok|not ok)\s+\d+\b|^\s*---\s(PASS|FAIL):|^\S+::\S+.*\b(PASSED|FAILED)\b/;

const SUMMARY_LINE =
  /^\s*(Test (Suites|Files)\b:?|Tests\b:?|Snapshots\b:?|Time\b:?|Ran all test suites|Start at\b:?|Duration\b:?)|^\d+ (passed|failed|examples?|tests?)\b|^(OK|FAILURES!)\b|\d+ examples?,\s*\d+ failures?|assertions?\)\s*$|^(ok|FAIL)\s+\S+\s+[\d.]+s$|^\d+ (errors?|passed|failed) in [\d.]+s$|={3,}\s*(FAILURES|ERRORS|short test summary)/i;

const PASS_NOISE =
  /^\s*(PASS\s+\S|✓|✔|--- PASS:|ok\s+\d+\b)|^\s*(=== RUN|RUN\s+\S)|^\S+::\S+.*\bPASSED\b/;

const DOT_PROGRESS = /^[.EFsx]{3,}(\s*\[\s*\d{1,3}%\])?$/;

const FAIL_MARK = /[✕✗×]|(^|\s)not ok\b/i;

function isProtected(line) {
  return isSevere(line) || FAIL_MARK.test(line);
}

function isPassNoiseLine(line) {
  return PASS_NOISE.test(line) || DOT_PROGRESS.test(line);
}

export function detect(lines) {
  const body = lines.filter((l) => l.trim() !== "");
  if (!body.length) return 0;

  const caseHits = body.filter((l) => CASE_MARK.test(l)).length;
  const progressHits = body.filter((l) => DOT_PROGRESS.test(l)).length;
  const summaryHit = body.some((l) => SUMMARY_LINE.test(l));
  const caseRatio = caseHits / body.length;

  let confidence = 0;
  if (summaryHit) confidence += 0.55;
  if (caseHits > 0) confidence += Math.min(0.45, caseRatio * 6 + 0.15);
  if (progressHits > 0) confidence += 0.15;
  return Math.min(1, confidence);
}

function chunksOf(lines) {
  const chunks = [];
  let current = [];
  for (const line of lines) {
    if (line.trim() === "") {
      if (current.length) chunks.push(current);
      chunks.push([line]);
      current = [];
    } else {
      current.push(line);
    }
  }
  if (current.length) chunks.push(current);
  return chunks;
}

function normalize(text) {
  return text
    .replace(/["'][^"']*["']/g, "S")
    .replace(/\b[0-9a-f]{7,40}\b/g, "H")
    .replace(/\d+/g, "N");
}

/** Compresses a test-report style output: keeps every failure block whole, drops the pass flood. */
export function compress(lines) {
  const chunks = chunksOf(lines);
  const seen = new Map();
  const out = [];
  let dropped = 0;
  let collapsedGroups = 0;

  for (const chunk of chunks) {
    if (chunk.length === 1 && chunk[0].trim() === "") {
      out.push(...chunk);
      continue;
    }

    if (chunk.every(isPassNoiseLine)) {
      dropped += chunk.length;
      continue;
    }

    const key = normalize(chunk.join("\n"));
    const seenCount = seen.get(key) || 0;
    seen.set(key, seenCount + 1);

    if (seenCount === 0) {
      out.push(...chunk);
      continue;
    }

    const protectedLines = chunk.filter(isProtected);
    if (protectedLines.length) {
      out.push(...protectedLines);
      dropped += chunk.length - protectedLines.length;
    } else {
      dropped += chunk.length;
    }
    if (chunk.length > protectedLines.length) collapsedGroups++;
  }

  const text = out.join("\n");
  const note = dropped
    ? `${dropped} linha(s) de caso/repeticao cortada(s)${collapsedGroups ? ` (${collapsedGroups} bloco(s) repetido(s) resumido(s))` : ""}`
    : "";
  return { text, dropped, note };
}
