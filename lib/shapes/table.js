import { isSevere } from "../learn.js";

export const name = "table";

const HEALTHY = /^(Up|Running|Ready|Active|Completed|Succeeded|Bound|healthy)\b/i;
const RATIO = /^(\d+)\/(\d+)$/;

function columnIsHealthy(token, value) {
  const m = RATIO.exec(value);
  if (m) return m[1] === m[2];
  if (/^(STATUS|STATE)$/i.test(token)) return HEALTHY.test(value);
  return true;
}
const HEADER_WORD = /^[A-Z0-9][A-Z0-9 _/.%-]*$/;

function splitColumns(line) {
  return line.split(/\s{2,}/).filter(Boolean);
}

function isHeaderLine(line) {
  const cols = splitColumns(line.trim());
  return cols.length >= 2 && cols.every((c) => HEADER_WORD.test(c));
}

function columnStarts(line, tokens) {
  const starts = [];
  let from = 0;
  for (const t of tokens) {
    const idx = line.indexOf(t, from);
    starts.push(idx);
    from = idx + t.length;
  }
  return starts;
}

function findHeader(lines) {
  const limit = Math.min(lines.length, 6);
  for (let i = 0; i < limit; i++) if (isHeaderLine(lines[i])) return i;
  return -1;
}

function boundaryOk(line, starts) {
  for (let c = 1; c < starts.length; c++) {
    if (line.length <= starts[c]) continue;
    if (line[starts[c] - 1] !== " ") return false;
  }
  return true;
}

function sliceColumn(line, start, end) {
  return (end === undefined ? line.slice(start) : line.slice(start, end)).trim();
}

/** Confidence 0..1 that lines form a columnar table with an all-caps header. */
export function detect(lines) {
  const idx = findHeader(lines);
  if (idx === -1) return 0;
  const header = lines[idx];
  const tokens = splitColumns(header.trim());
  const starts = columnStarts(header, tokens);
  const data = lines.slice(idx + 1).filter((l) => l.trim());
  if (!data.length) return 0.5;
  let ok = 0;
  for (const l of data) if (boundaryOk(l, starts)) ok++;
  return Math.min(1, 0.35 + (ok / data.length) * 0.65);
}

/**
 * Drops columns whose value never varies across rows, collapses padding to a
 * single space, and leaves any row whose status column is not a healthy one
 * (or whose line trips isSevere) untouched and complete.
 */
export function compress(lines) {
  const idx = findHeader(lines);
  const original = lines.join("\n");
  if (idx === -1) return { text: original, dropped: 0, note: "" };

  const before = lines.slice(0, idx);
  const header = lines[idx];
  const tokens = splitColumns(header.trim());
  const starts = columnStarts(header, tokens);
  const ends = starts.slice(1);

  const rows = [];
  let after = [];
  let i = idx + 1;
  for (; i < lines.length; i++) {
    const line = lines[i];
    if (!line.trim() || (rows.length && !boundaryOk(line, starts))) {
      after = lines.slice(i);
      break;
    }
    rows.push(line);
  }

  const stateCols = tokens
    .map((t, c) => (/^(STATUS|STATE|READY)$/i.test(t) ? c : -1))
    .filter((c) => c !== -1);
  const fullSurvive = rows.map((line) => {
    if (isSevere(line)) return true;
    if (!stateCols.length) return false;
    return stateCols.some((c) => !columnIsHealthy(tokens[c], sliceColumn(line, starts[c], ends[c])));
  });

  const healthy = rows.filter((_, r) => !fullSurvive[r]);
  const constantValue = tokens.map((_, c) => {
    if (healthy.length < 3) return null;
    const values = new Set(healthy.map((line) => sliceColumn(line, starts[c], ends[c])));
    return values.size === 1 ? [...values][0] : null;
  });

  let survivingCols = tokens.map((_, c) => c).filter((c) => constantValue[c] === null);
  if (survivingCols.length === 0) survivingCols = [0];
  const droppedCols = tokens.map((_, c) => c).filter((c) => !survivingCols.includes(c));

  const constantNote = droppedCols.map((c) => `${tokens[c]}=${constantValue[c]}`).join(" ");
  const outHeader =
    survivingCols.map((c) => tokens[c]).join(" ") + (constantNote ? `   [iguais em todas: ${constantNote}]` : "");
  const reduce = (line) => survivingCols.map((c) => sliceColumn(line, starts[c], ends[c])).join(" ");
  const rest = survivingCols.slice(1);
  const outRows = [];
  const groups = new Map();
  for (let r = 0; r < rows.length; r++) {
    if (fullSurvive[r]) {
      outRows.push(rows[r]);
      continue;
    }
    const key = rest.map((c) => sliceColumn(rows[r], starts[c], ends[c])).join(" ");
    if (!groups.has(key)) {
      groups.set(key, { slot: outRows.length, names: [] });
      outRows.push(null);
    }
    groups.get(key).names.push(sliceColumn(rows[r], starts[survivingCols[0]], ends[survivingCols[0]]));
    if (groups.get(key).names.length === 1) outRows[groups.get(key).slot] = reduce(rows[r]);
  }
  for (const [key, g] of groups) {
    if (g.names.length < 4) {
      const [, ...others] = g.names;
      let at = g.slot + 1;
      for (const n of others) outRows.splice(at++, 0, `${n} ${key}`);
      continue;
    }
    outRows[g.slot] = `${g.names.length}x ${key}  ${g.names.join(" ")}`;
  }

  const text = [...before, outHeader, ...outRows, ...after].join("\n");
  if (text.length >= original.length) return { text: original, dropped: 0, note: "" };

  const note = droppedCols.length
    ? `colunas sem variacao removidas (${droppedCols.map((c) => `${tokens[c]}=${constantValue[c]}`).join(", ")}); padding colapsado`
    : "padding colapsado";

  return { text, dropped: droppedCols.length, note };
}
