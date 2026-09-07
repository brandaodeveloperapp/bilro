/**
 * Shape of a static checker's own output: tsc, mypy, ruff, rubocop, phpstan,
 * golangci-lint, eslint, cargo, mvn, gradlew, sbt, dotnet, next build.
 * Structurally these agree on `path:line:col: message`, sometimes with the
 * rule/code trailing or leading, sometimes with the path on its own header
 * line above indented `line:col` entries (eslint's stylish formatter).
 * Compression groups repeats of the same rule/code and keeps one concrete
 * example per group with its path:line — it never hides that a group exists.
 */
import { isSevere } from "../learn.js";

export const name = "diagnostics";

const COLON_LOC =
  /^(?!\s*at\b)(?<path>[^\s:()][^:()]*[./][^\s:()]*):(?<line>\d+):(?<col>\d+):?\s*[-–]?\s*(?<msg>.+)$/;
const MYPY_LOC =
  /^(?!\s*at\b)(?<path>[^\s:()][^:()]*[./][^\s:()]*):(?<line>\d+):\s*(?<severity>error|warning|note):\s*(?<msg>.+)$/i;
const PAREN_LOC = /^(?<path>[^\s():]+)\((?<line>\d+),(?<col>\d+)\):\s*(?<msg>.+)$/;
const HEADER_PATH = /^\.?[./][^\s:]*\.[a-zA-Z]{1,10}$|^[^\s:]+\.[a-zA-Z]{1,10}$/;
const INDENT_LOC = /^\s*(?<line>\d+):(?<col>\d+)\s+(?<severity>Warning|Error|error|warning)\b:?\s*(?<msg>.+)$/;
const SUMMARY_LINE = /^(Found \d+ error|\d+ problems?\s*\(|^\[\*\]|BUILD (SUCCESS|FAILURE))/i;

function matchAny(line) {
  return line.match(COLON_LOC) || line.match(MYPY_LOC) || line.match(PAREN_LOC);
}

function ruleCodeOf(line, msg) {
  const codeInMsg = msg.match(/\b([A-Z]{1,10}\d{3,5})\b/);
  if (codeInMsg) return codeInMsg[1];
  const trailing = msg.match(/([\w-]+(?:\/[\w-]+)+)\s*$/);
  if (trailing) return trailing[1];
  const bracket = line.match(/\[\*?\]?\s*([\w-]+(?:\/[\w-]+)*)\s*$/);
  if (bracket) return bracket[1];
  return msg.slice(0, 40).trim();
}

export function detect(lines) {
  const body = lines.filter((l) => l.trim() !== "");
  if (!body.length) return 0;

  let locHits = 0;
  let indentHits = 0;
  let headerCount = 0;
  let summaryHit = false;
  for (const line of body) {
    if (matchAny(line)) locHits++;
    else if (INDENT_LOC.test(line)) indentHits++;
    else if (HEADER_PATH.test(line.trim())) headerCount++;
    if (SUMMARY_LINE.test(line)) summaryHit = true;
  }

  const totalHits = locHits + indentHits;
  if (!totalHits) return 0;

  const ratio = totalHits / body.length;
  let confidence = Math.min(0.85, ratio * 3 + 0.2);
  if (indentHits > 0 && headerCount > 0) confidence = Math.min(1, confidence + 0.15);
  if (summaryHit) confidence = Math.min(1, confidence + 0.1);
  return Math.min(1, confidence);
}

function parseEntries(lines) {
  const entries = [];
  const headerIndexByLine = new Map();
  let currentHeaderIndex = null;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const m = matchAny(line);
    if (m) {
      const { path, line: lineNo, col, msg } = m.groups;
      entries.push({
        path,
        loc: `${path}:${lineNo}${col ? `:${col}` : ""}`,
        code: ruleCodeOf(line, msg || line),
        severe: isSevere(line),
        index: i,
        headerIndex: null,
      });
      continue;
    }
    if (INDENT_LOC.test(line)) {
      const m2 = line.match(INDENT_LOC).groups;
      const path = currentHeaderIndex === null ? "?" : lines[currentHeaderIndex].trim();
      entries.push({
        path,
        loc: `${path}:${m2.line}:${m2.col}`,
        code: ruleCodeOf(line, m2.msg),
        severe: isSevere(line),
        index: i,
        headerIndex: currentHeaderIndex,
      });
      continue;
    }
    if (HEADER_PATH.test(line.trim())) {
      currentHeaderIndex = i;
      headerIndexByLine.set(i, true);
    }
  }
  return { entries, headerIndexByLine };
}

/** Compresses a diagnostics-list style output: groups repeats by rule/code, keeps one example each. */
export function compress(lines) {
  const { entries, headerIndexByLine } = parseEntries(lines);
  if (!entries.length) return { text: lines.join("\n"), dropped: 0, note: "" };

  const entryByIndex = new Map(entries.map((e) => [e.index, e]));

  const groups = new Map();
  for (const e of entries) {
    if (!groups.has(e.code)) groups.set(e.code, []);
    groups.get(e.code).push(e);
  }

  const keepEntryIndex = new Set();
  for (const [, group] of groups) {
    keepEntryIndex.add(group[0].index);
    for (const e of group) if (e.severe) keepEntryIndex.add(e.index);
  }

  const keepLine = new Set();
  for (const i of keepEntryIndex) {
    keepLine.add(i);
    const headerIndex = entryByIndex.get(i).headerIndex;
    if (headerIndex !== null) keepLine.add(headerIndex);
  }

  const out = [];
  let dropped = 0;
  const summarized = new Set();

  for (let i = 0; i < lines.length; i++) {
    if (keepLine.has(i)) {
      out.push(lines[i]);
      continue;
    }
    const entry = entryByIndex.get(i);
    if (!entry) {
      if (headerIndexByLine.has(i)) continue;
      out.push(lines[i]);
      continue;
    }
    dropped++;
    if (!summarized.has(entry.code)) {
      summarized.add(entry.code);
      const group = groups.get(entry.code);
      const last = group[group.length - 1];
      out.push(`  … ${group.length - 1} outra(s) ocorrencia(s) de ${entry.code} (ex: ${last.loc})`);
    }
  }

  const multiGroups = [...groups.values()].filter((g) => g.length > 1).length;
  const note = dropped
    ? `${dropped} linha(s) agrupada(s) em ${multiGroups} regra(s) repetida(s), ${groups.size} regra(s) no total`
    : "";
  return { text: out.join("\n"), dropped, note };
}
