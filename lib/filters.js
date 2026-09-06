/**
 * Output filters. Each rule keeps the lines a reader acts on and drops the
 * ones that only prove the tool ran.
 */

const drop = (...res) => (lines) => lines.filter((l) => !res.some((re) => re.test(l)));

const RULES = [
  {
    name: "jest",
    when: /\bjest\b|\bvitest\b/,
    apply: drop(
      /^\s*at /,
      /^\s*console\.(log|warn|error)$/,
      /node_modules\//,
      /^\s*✓ /,
      /^\s*✔ /,
      /^\s*PASS /,
      /^\s*at Object\./,
      /asyncGeneratorStep|_next|new Promise|processTicksAndRejections/,
    ),
  },
  {
    name: "gradle",
    when: /gradlew|gradle /,
    apply: drop(/^> Task .* (UP-TO-DATE|NO-SOURCE)$/, /^Download /, /^\s*$/),
  },
  {
    name: "npm",
    when: /\bnpm (install|ci|i)\b|\byarn\b|\bpnpm i\b/,
    apply: drop(/^npm warn/, /^\s*added \d+ packages/, /deprecated/, /^\s*$/),
  },
  {
    name: "tsc",
    when: /\btsc\b/,
    apply: (lines) => lines.filter((l) => /error TS|\.tsx?\(\d+,\d+\)/.test(l) || /^\s*$/.test(l) === false),
  },
  {
    name: "kubectl",
    when: /\bkubectl\b/,
    apply: drop(/^\s*$/, /Warning: /),
  },
  {
    name: "git-status",
    when: /\bgit status\b/,
    apply: drop(/^\s*\(use "git /, /^\s*$/, /^On branch/, /^Your branch is up to date/),
  },
  {
    name: "docker",
    when: /\bdocker\b/,
    apply: drop(/^\s*$/, /Pulling fs layer|Waiting|Verifying Checksum|Download complete|Already exists/),
  },
  {
    name: "fastlane",
    when: /\bfastlane\b/,
    apply: drop(/^\s*▸/, /^\[.*\]: ▸/, /warning:/i, /^\s*$/),
  },
];

/** Collapses runs of identical lines into one, marked with the repeat count. */
export function collapseRepeats(lines) {
  const out = [];
  let last = null;
  let count = 0;
  const flush = () => {
    if (last === null) return;
    out.push(count > 1 ? `${last}   (×${count})` : last);
  };
  for (const line of lines) {
    if (line === last) count++;
    else {
      flush();
      last = line;
      count = 1;
    }
  }
  flush();
  return out;
}

export function ruleFor(command) {
  return RULES.find((r) => r.when.test(command)) || null;
}

export function filter(command, output, { keepTail = 40 } = {}) {
  const before = output.length;
  const rule = ruleFor(command);
  let lines = output.split("\n");
  if (rule) lines = rule.apply(lines);
  lines = collapseRepeats(lines);
  let text = lines.join("\n").trim();
  if (!text) text = output.split("\n").slice(-keepTail).join("\n").trim();
  return { text, rule: rule ? rule.name : null, before, after: text.length };
}

export const rules = RULES.map((r) => r.name);
