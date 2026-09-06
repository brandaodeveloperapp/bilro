import { loadMemories } from "./memory.js";

const LINK = /\[\[([^\]|#]+)(?:#[^\]|]+)?(?:\|[^\]]+)?\]\]/g;

export function linksOf(memory) {
  const found = new Set();
  for (const m of memory.body.matchAll(LINK)) found.add(m[1].trim());
  return [...found];
}

/** Who points at whom, so a note can be judged by its place in the web. */
export function build(dir) {
  const memories = loadMemories(dir);
  const byName = new Map(memories.map((m) => [m.name, m]));
  const out = new Map();
  const back = new Map();
  for (const m of memories) {
    const targets = linksOf(m);
    out.set(m.name, targets);
    for (const t of targets) {
      if (!back.has(t)) back.set(t, []);
      back.get(t).push(m.name);
    }
  }
  return { memories, byName, out, back };
}

export function lint(dir) {
  const g = build(dir);
  const broken = [];
  const orphans = [];
  const hubs = [];

  for (const [from, targets] of g.out) {
    for (const t of targets) if (!g.byName.has(t)) broken.push({ from, to: t });
  }
  for (const m of g.memories) {
    const incoming = (g.back.get(m.name) || []).length;
    const outgoing = (g.out.get(m.name) || []).length;
    if (!incoming && !outgoing) orphans.push(m.name);
    if (incoming >= 4) hubs.push({ name: m.name, incoming });
  }
  hubs.sort((a, b) => b.incoming - a.incoming);

  return { total: g.memories.length, broken, orphans, hubs: hubs.slice(0, 5) };
}
