import { isSevere } from "../learn.js";

export const name = "install-log";

const PROGRESS_RE =
  /^npm (verbose|info|http|timing)\b|^\s*(Collecting|Downloading|Using cached|Requirement already satisfied|Building wheel|Preparing metadata|Resolving|Fetching)\b|^\s*[+-]\s+\S+==\S+|^\s*\[notice\]/i;
const WARN_RE = /^npm warn\b|^\s*warning:/i;
const FUNDING_RE = /looking for funding|run `npm fund`/i;
const SUMMARY_RE =
  /\b(added|removed|changed)\s+\d+\s+packages?\b|\baudited\s+\d+\s+packages?\b|\bup to date in\b|Successfully installed|\bResolved\s+\d+\s+packages?\b|\bPrepared\s+\d+\s+packages?\b|\bInstalled\s+\d+\s+packages?\b|found\s+\d+\s+vulnerabilit/i;

/** Strips a bare carriage-return progress bar down to its trailing state, if any. */
function stripCarriageReturn(line) {
  if (!line.includes("\r")) return line;
  const segments = line.split("\r");
  return segments[segments.length - 1];
}

function classify(lines) {
  const nonEmpty = lines.map(stripCarriageReturn).filter((l) => l.trim());
  const n = nonEmpty.length;
  if (n < 2) return 0;
  const progress = nonEmpty.filter((l) => PROGRESS_RE.test(l)).length;
  const warn = nonEmpty.filter((l) => WARN_RE.test(l)).length;
  const summary = nonEmpty.filter((l) => SUMMARY_RE.test(l)).length;
  if (summary === 0) return 0;
  return (progress + warn + summary) / n;
}

export function detect(lines) {
  return classify(lines);
}

export function compress(lines) {
  const kept = [];
  let droppedProgress = 0;
  let droppedFunding = 0;
  let droppedBlank = 0;

  for (const raw of lines) {
    const line = stripCarriageReturn(raw);
    if (!line.trim()) {
      droppedBlank++;
      continue;
    }
    if (isSevere(line) || SUMMARY_RE.test(line) || WARN_RE.test(line)) {
      kept.push(line);
      continue;
    }
    if (FUNDING_RE.test(line)) {
      droppedFunding++;
      continue;
    }
    if (PROGRESS_RE.test(line)) {
      droppedProgress++;
      continue;
    }
    kept.push(line);
  }

  const text = kept.join("\n");
  const dropped = lines.length - kept.length;
  const note = `install-log: ${droppedProgress} linha(s) de progresso, ${droppedFunding} de funding e ${droppedBlank} em branco cortadas`;
  return { text, dropped, note };
}
