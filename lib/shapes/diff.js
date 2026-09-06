import { isSevere } from "../learn.js";

export const name = "diff";

const FILE_HEADER = /^diff --(git|cc) /;
const HUNK = /^@@[@ ]/;
const META = /^(old mode|new mode|deleted file mode|new file mode|similarity index|dissimilarity index|rename from|rename to|copy from|copy to|Binary files|index |--- |\+\+\+ )/;
const CHANGE_LINE = /^[+-]/;

const STATUS_SECTION = {
  "Changes to be committed:": "staged",
  "Changes not staged for commit:": "unstaged",
  "Untracked files:": "untracked",
  "Unmerged paths:": "conflict",
};
const STATUS_ENTRY = /^(modified|deleted|new file|renamed|copied|both modified|both added|added by us|deleted by us|deleted by them|added by them):\s*(.+)$/;

const BUDGET_CHARS = 10000;

function countStructure(lines) {
  let fileHeaders = 0;
  let hunks = 0;
  let plusMinus = 0;
  let statusMarkers = 0;
  for (const l of lines) {
    if (FILE_HEADER.test(l)) fileHeaders++;
    else if (HUNK.test(l)) hunks++;
    else if (CHANGE_LINE.test(l) && !META.test(l)) plusMinus++;
    else if (STATUS_ENTRY.test(l.trim())) statusMarkers++;
    else if (STATUS_SECTION[l.trim()]) statusMarkers += 2;
  }
  return { fileHeaders, hunks, plusMinus, statusMarkers };
}

/** Confidence 0..1 that lines are a unified diff or a git-status change list. */
export function detect(lines) {
  const { fileHeaders, hunks, plusMinus, statusMarkers } = countStructure(lines);
  if (fileHeaders > 0 || hunks > 0) return Math.min(1, 0.6 + Math.min(hunks, 5) * 0.06 + Math.min(plusMinus, 10) * 0.01);
  if (statusMarkers >= 2) return Math.min(1, 0.65 + statusMarkers * 0.03);
  return 0;
}

function compressUnifiedDiff(lines) {
  const out = [];
  const fileBlockStarts = [];
  let inHunk = false;
  let pendingDrop = 0;
  let droppedTotal = 0;
  let fileCount = 0;
  let hunkCount = 0;

  const flushPending = () => {
    if (pendingDrop === 0) return;
    out.push(`      … ${pendingDrop} linha${pendingDrop > 1 ? "s" : ""} de contexto omitida${pendingDrop > 1 ? "s" : ""}`);
    droppedTotal += pendingDrop;
    pendingDrop = 0;
  };

  for (const line of lines) {
    if (FILE_HEADER.test(line)) {
      flushPending();
      fileBlockStarts.push(out.length);
      fileCount++;
      inHunk = false;
      out.push(line);
      continue;
    }
    if (!inHunk && META.test(line)) {
      out.push(line);
      continue;
    }
    if (HUNK.test(line)) {
      flushPending();
      hunkCount++;
      inHunk = true;
      out.push(line);
      continue;
    }
    if (inHunk) {
      if (CHANGE_LINE.test(line) || isSevere(line)) {
        flushPending();
        out.push(line);
      } else {
        pendingDrop++;
      }
      continue;
    }
    out.push(line);
  }
  flushPending();

  let text = out.join("\n");
  let note = `${droppedTotal} linhas de contexto removidas em ${hunkCount} hunks (${fileCount} arquivos)`;

  if (text.length > BUDGET_CHARS && fileBlockStarts.length > 1) {
    let shown = fileBlockStarts.length;
    for (let k = fileBlockStarts.length - 1; k >= 1; k--) {
      const slice = out.slice(0, fileBlockStarts[k]).join("\n");
      if (slice.length <= BUDGET_CHARS) {
        text = slice;
        shown = k;
        break;
      }
      shown = 0;
    }
    const omitted = fileCount - shown;
    if (omitted > 0) {
      note += ` | orcamento estourado: mostrando ${shown} de ${fileCount} arquivos, ${omitted} arquivos omitidos por corte de orcamento`;
    }
  }

  return { text, dropped: droppedTotal, note };
}

function compressStatus(lines) {
  const out = [];
  let section = null;
  let bucket = new Map();
  let rawEntries = [];
  let droppedTotal = 0;

  const flushSection = () => {
    if (!section) return;
    if (section === "conflict") {
      for (const e of rawEntries) out.push(`  ${e}`);
    } else {
      for (const [type, entries] of bucket) out.push(`  ${type} (${entries.length}): ${entries.join(", ")}`);
    }
    bucket = new Map();
    rawEntries = [];
  };

  for (const line of lines) {
    const trimmed = line.trim();
    if (STATUS_SECTION[trimmed]) {
      flushSection();
      section = STATUS_SECTION[trimmed];
      out.push(trimmed);
      continue;
    }
    if (!trimmed) {
      droppedTotal++;
      continue;
    }
    if (trimmed.startsWith("(")) {
      droppedTotal++;
      continue;
    }
    if (section && line.startsWith("\t")) {
      if (section === "conflict") {
        rawEntries.push(trimmed);
        continue;
      }
      const m = STATUS_ENTRY.exec(trimmed);
      const type = m ? m[1] : "untracked";
      const path = m ? m[2] : trimmed;
      if (!bucket.has(type)) bucket.set(type, []);
      bucket.get(type).push(path);
      continue;
    }
    flushSection();
    section = null;
    out.push(line);
  }
  flushSection();

  return { text: out.join("\n"), dropped: droppedTotal, note: `${droppedTotal} linhas de aviso/em branco removidas` };
}

/**
 * Keeps every file header and every +/- line; drops unchanged context lines
 * (or collapses git-status hint lines), grouping status entries by change
 * type. A merge conflict or isSevere line is never among what is dropped.
 * If the result still overruns the budget, whole file blocks are cut from
 * the end and the note says exactly how many files and hunks were left out.
 */
export function compress(lines) {
  const original = lines.join("\n");
  const { fileHeaders, hunks } = countStructure(lines);
  const r = fileHeaders > 0 || hunks > 0 ? compressUnifiedDiff(lines) : compressStatus(lines);
  if (r.text.length >= original.length) return { text: original, dropped: 0, note: "" };
  return r;
}
