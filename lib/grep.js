const HIT = /^([^:]+):(\d+):(.*)$/;

/**
 * Groups matches by file and collapses identical match text into one line with
 * a count. The matched content is the whole point of a search, so it is never
 * truncated away; only the repetition around it is.
 */
export function compress(text, { perFile = 12 } = {}) {
  const lines = text.split("\n").filter((l) => l.trim());
  const files = new Map();
  const passthrough = [];

  for (const line of lines) {
    const m = HIT.exec(line);
    if (!m) {
      passthrough.push(line);
      continue;
    }
    const [, file, no, body] = m;
    if (!files.has(file)) files.set(file, new Map());
    const key = body.trim();
    const seen = files.get(file);
    if (!seen.has(key)) seen.set(key, { first: no, count: 0, lines: [] });
    const e = seen.get(key);
    e.count++;
    if (e.lines.length < 3) e.lines.push(no);
  }

  if (!files.size) return { text, files: 0, hits: 0, dropped: 0 };

  const out = [];
  let hits = 0;
  for (const [file, seen] of files) {
    const entries = [...seen.entries()];
    const total = entries.reduce((n, [, e]) => n + e.count, 0);
    hits += total;
    out.push(`${file}  ${total}`);
    for (const [body, e] of entries.slice(0, perFile))
      out.push(`  ${e.lines.join(",")}${e.count > e.lines.length ? `+${e.count - e.lines.length}` : ""}: ${body}`);
    if (entries.length > perFile) out.push(`  … ${entries.length - perFile} outros trechos neste arquivo`);
  }
  const compressed = [...passthrough, ...out].join("\n");
  return {
    text: compressed,
    files: files.size,
    hits,
    dropped: lines.length - compressed.split("\n").length,
  };
}
