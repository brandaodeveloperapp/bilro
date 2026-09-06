import { isSevere } from "../learn.js";

export const name = "listing";

const LS_ENTRY_RE =
  /^([-dlbcps][-rwxXsStT]{9}[@+]?)\s+(\d+)\s+(\S+)\s+(\S+)\s+(\d+)\s+(\S+\s+\S+\s+\S+)\s+(.+)$/;
const TOTAL_RE = /^total\s+\d+$/;
const HEADER_RE = /^\S[^\s:]*:$/;
const GREP_HIT_RE = /^(\S+):(\d+):(.*)$/;
const TREE_RE = /^[\s│]*[├└]──\s+\S/;
const WC_ENTRY_RE = /^\s*\d+\s+\S+$/;

const EXT_THRESHOLD = 30;

/** True when a trimmed line is one bare filesystem path with no spaces or `=`. */
function isPathLine(line) {
  const t = line.trim();
  return t.length > 0 && !t.includes(" ") && t.includes("/") && !t.includes("=");
}

/** True for a grep/rg style `path:line:content` hit, guarded against multi-colon noise. */
function isGrepHit(line) {
  if (!GREP_HIT_RE.test(line)) return false;
  return (line.match(/:/g) || []).length <= 6;
}

function classify(lines) {
  const nonEmpty = lines.filter((l) => l.trim());
  const n = nonEmpty.length;
  if (n < 2) return { mode: "none", ratio: 0 };
  const scored = [
    ["ls", nonEmpty.filter((l) => LS_ENTRY_RE.test(l) || TOTAL_RE.test(l) || HEADER_RE.test(l.trim())).length],
    ["grep", nonEmpty.filter(isGrepHit).length],
    ["tree", nonEmpty.filter((l) => TREE_RE.test(l)).length],
    ["path", nonEmpty.filter(isPathLine).length],
    ["wc", nonEmpty.filter((l) => WC_ENTRY_RE.test(l)).length],
  ];
  scored.sort((a, b) => b[1] - a[1]);
  const [mode, hits] = scored[0];
  return { mode, ratio: hits / n };
}

export function detect(lines) {
  return classify(lines).ratio;
}

function extensionOf(path) {
  const base = path.split("/").pop();
  const m = base.match(/\.[A-Za-z0-9]+$/);
  return m ? m[0].toLowerCase() : "(sem extensao)";
}

function groupByExtension(paths) {
  const counts = new Map();
  for (const p of paths) counts.set(extensionOf(p), (counts.get(extensionOf(p)) || 0) + 1);
  return [...counts.entries()].sort((a, b) => b[1] - a[1]);
}

function commonPrefix(paths) {
  if (paths.length < 2) return "";
  let prefix = paths[0];
  for (const p of paths.slice(1)) {
    let i = 0;
    while (i < prefix.length && i < p.length && prefix[i] === p[i]) i++;
    prefix = prefix.slice(0, i);
    if (!prefix) return "";
  }
  const cut = prefix.lastIndexOf("/");
  return cut > 0 ? prefix.slice(0, cut + 1) : "";
}

function compressPathList(lines) {
  const paths = lines.filter((l) => l.trim()).map((l) => l.trim());
  const prefix = commonPrefix(paths);
  const rel = prefix ? paths.map((p) => p.slice(prefix.length)) : paths;
  let body;
  let note;
  if (paths.length >= EXT_THRESHOLD) {
    const groups = groupByExtension(rel);
    body = groups.map(([ext, count]) => `${ext}: ${count} arquivo${count === 1 ? "" : "s"}`).join("\n");
    note = `${paths.length} caminhos agrupados em ${groups.length} extensao(oes)${prefix ? `, prefixo comum removido (${prefix})` : ""}`;
  } else {
    body = rel.join("\n");
    note = prefix ? `prefixo comum removido: ${prefix}` : "sem prefixo comum a remover";
  }
  return { text: body, dropped: lines.length - body.split("\n").length, note };
}

function compressGrep(lines) {
  const groups = new Map();
  const order = [];
  const passthrough = [];
  let hits = 0;
  for (const line of lines) {
    const m = line.match(GREP_HIT_RE);
    if (m && isGrepHit(line)) {
      hits++;
      const [, file, num, rest] = m;
      if (!groups.has(file)) {
        groups.set(file, []);
        order.push(file);
      }
      groups.get(file).push(`  ${num}: ${rest}`);
    } else if (line.trim()) {
      passthrough.push(line);
    }
  }
  const out = [...passthrough];
  for (const file of order) {
    out.push(`${file}:`, ...groups.get(file));
  }
  const text = out.join("\n");
  return {
    text,
    dropped: lines.length - out.length,
    note: `${hits} hit(s) agrupados em ${order.length} arquivo(s)`,
  };
}

function parseLsBlocks(lines) {
  const blocks = [];
  let cur = [];
  for (const line of [...lines, ""]) {
    if (!line.trim()) {
      if (cur.length) blocks.push(cur);
      cur = [];
    } else {
      cur.push(line);
    }
  }
  return blocks.map((block) => {
    let header = null;
    let rest = block;
    if (HEADER_RE.test(block[0].trim()) && TOTAL_RE.test((block[1] || "").trim())) {
      header = block[0].trim().slice(0, -1);
      rest = block.slice(2);
    } else if (TOTAL_RE.test(block[0].trim())) {
      rest = block.slice(1);
    }
    const entries = [];
    const unmatched = [];
    for (const line of rest) {
      const m = line.match(LS_ENTRY_RE);
      if (m) {
        const [, perm, links, owner, group, size, , entryName] = m;
        if (entryName === "." || entryName === "..") continue;
        entries.push({ perm, links, owner, group, size, name: entryName });
      } else if (line.trim()) {
        unmatched.push(line);
      }
    }
    return { header, entries, unmatched };
  });
}

function compressLs(lines) {
  const blocks = parseLsBlocks(lines);
  const allEntries = blocks.flatMap((b) => b.entries);
  const owners = new Set(allEntries.map((e) => e.owner));
  const groupsSet = new Set(allEntries.map((e) => e.group));
  const perms = new Set(allEntries.map((e) => e.perm));
  const dropOwner = owners.size === 1;
  const dropGroup = groupsSet.size === 1;
  const dropPerm = perms.size === 1;

  const headers = blocks.map((b) => b.header).filter(Boolean);
  const prefix = commonPrefix(headers);

  const out = [];
  let droppedFields = 0;
  for (const block of blocks) {
    const relHeader = block.header ? (prefix ? block.header.slice(prefix.length) || "." : block.header) : null;
    if (relHeader !== null) out.push(`${relHeader}:`);
    if (block.entries.length >= EXT_THRESHOLD) {
      const groups = groupByExtension(block.entries.map((e) => e.name));
      for (const [ext, count] of groups) out.push(`  ${ext}: ${count} arquivo${count === 1 ? "" : "s"}`);
      droppedFields += block.entries.length - groups.length;
    } else {
      for (const e of block.entries) {
        const cols = [];
        if (!dropPerm) cols.push(e.perm);
        cols.push(e.size);
        if (!dropOwner) cols.push(e.owner);
        if (!dropGroup) cols.push(e.group);
        cols.push(e.name);
        out.push(`  ${cols.join(" ")}`);
      }
    }
    out.push(...block.unmatched);
  }
  if (dropOwner) droppedFields += allEntries.length;
  if (dropGroup) droppedFields += allEntries.length;
  if (dropPerm) droppedFields += allEntries.length;

  const notes = [];
  if (dropOwner || dropGroup) notes.push("dono/grupo identicos em tudo, coluna removida");
  if (dropPerm) notes.push("permissao identica em tudo, coluna removida");
  if (prefix) notes.push(`raiz comum removida: ${prefix}`);

  const text = out.join("\n");
  return {
    text,
    dropped: lines.length - out.length,
    note: notes.join("; ") || "sem coluna de baixa entropia para remover",
  };
}

/** Guarantees a line isSevere() flags as failure always survives, verbatim, in the output. */
function guardSevere(lines, result) {
  const missing = lines.filter((l) => l.trim() && isSevere(l) && !result.text.includes(l.trim()));
  if (!missing.length) return result;
  const text = [result.text, ...missing].filter(Boolean).join("\n");
  return {
    text,
    dropped: result.dropped - missing.length,
    note: `${result.note}; ${missing.length} linha(s) severa(s) preservada(s)`,
  };
}

export function compress(lines) {
  const { mode } = classify(lines);
  let result;
  if (mode === "grep") result = compressGrep(lines);
  else if (mode === "ls") result = compressLs(lines);
  else if (mode === "path" || mode === "wc") result = compressPathList(lines);
  else result = { text: lines.join("\n"), dropped: 0, note: "listing: forma reconhecida mas sem ganho seguro de compressao" };
  return guardSevere(lines, result);
}
