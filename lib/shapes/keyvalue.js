import { isSevere } from "../learn.js";

export const name = "keyvalue";

const KV_LINE_RE = /^(\s*)([A-Za-z_][\w.-]*)(\s*[:=]\s*)(.*)$/;

const SECRET_KEY_RE =
  /token|secret|password|passwd|senha|credential|authorization|bearer|api[_-]?key|access[_-]?key|private[_-]?key|client[_-]?secret|^KEY$|_KEY$/i;

const CREDENTIAL_IN_URL = /([a-z][a-z0-9+.-]*:\/\/)([^\s:@\/]*):([^\s@\/]+)@/gi;

const MASK = "***MASCARADO***";

const STRING_TRUNC = 120;
const ARRAY_COLLAPSE_THRESHOLD = 3;

/**
 * Masks a password embedded in a connection string. The key name says nothing
 * about it, so name-based masking alone lets a production credential through.
 */
export function maskEmbedded(value) {
  return String(value).replace(CREDENTIAL_IN_URL, (_, scheme, user) => `${scheme}${user}:${MASK}@`);
}

function parseWholeJSON(lines) {
  const text = lines.join("\n").trim();
  if (!text || (text[0] !== "{" && text[0] !== "[")) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

export function detect(lines) {
  if (parseWholeJSON(lines) !== null) return 1;
  const nonEmpty = lines.filter((l) => l.trim());
  if (nonEmpty.length < 2) return 0;
  const hits = nonEmpty.filter((l) => KV_LINE_RE.test(l)).length;
  return hits / nonEmpty.length;
}

function shapeSignature(item) {
  if (item === null) return "null";
  if (Array.isArray(item)) return "array";
  if (typeof item === "object") return Object.keys(item).sort().join(",");
  return typeof item;
}

function truncateString(value, stats) {
  if (value.length <= STRING_TRUNC || isSevere(value)) return value;
  stats.truncated++;
  return `${value.slice(0, STRING_TRUNC)}...(cortado, ${value.length} chars originais)`;
}

function shrinkValue(value, stats) {
  if (Array.isArray(value)) {
    if (value.length > ARRAY_COLLAPSE_THRESHOLD) {
      const sig = shapeSignature(value[0]);
      const homogeneous = value.every((v) => shapeSignature(v) === sig);
      if (homogeneous) {
        stats.collapsed += value.length - 1;
        return {
          "(resumo)": `${value.length} itens no mesmo formato, primeiro como esquema`,
          "(exemplo)": shrinkValue(value[0], stats),
        };
      }
    }
    return value.map((v) => shrinkValue(v, stats));
  }
  if (value !== null && typeof value === "object") {
    const out = {};
    for (const [key, v] of Object.entries(value)) {
      if (SECRET_KEY_RE.test(key)) {
        out[key] = MASK;
        stats.masked++;
      } else {
        const shrunk = shrinkValue(v, stats);
        if (typeof shrunk === "string") {
          const safe = maskEmbedded(shrunk);
          if (safe !== shrunk) stats.masked++;
          out[key] = safe;
        } else out[key] = shrunk;
      }
    }
    return out;
  }
  if (typeof value === "string") return truncateString(value, stats);
  return value;
}

function compressJSON(parsed) {
  const stats = { masked: 0, collapsed: 0, truncated: 0 };
  const shrunk = shrinkValue(parsed, stats);
  const text = JSON.stringify(shrunk, null, 2);
  const note = `keyvalue(json): ${stats.masked} valor(es) mascarado(s), ${stats.collapsed} item(ns) de array colapsado(s), ${stats.truncated} string(s) truncada(s)`;
  return { text, note, stats };
}

function compressKV(lines) {
  let masked = 0;
  let truncated = 0;
  const out = lines.map((line) => {
    const m = line.match(KV_LINE_RE);
    if (!m) return line;
    const [, indent, key, sep, rawValue] = m;
    let value = rawValue;
    if (SECRET_KEY_RE.test(key)) {
      masked++;
      value = MASK;
    } else {
      const safe = maskEmbedded(value);
      if (safe !== value) {
        masked++;
        value = safe;
      } else if (value.length > STRING_TRUNC && !isSevere(line)) {
        truncated++;
        value = `${value.slice(0, STRING_TRUNC)}...(cortado, ${value.length} chars originais)`;
      }
    }
    return `${indent}${key}${sep}${value}`;
  });
  const text = out.join("\n");
  const note = `keyvalue: ${masked} valor(es) mascarado(s), ${truncated} truncado(s)`;
  return { text, dropped: 0, note };
}

export function compress(lines) {
  const parsed = parseWholeJSON(lines);
  if (parsed !== null) {
    const { text, note } = compressJSON(parsed);
    const original = lines.join("\n");
    return { text, dropped: Math.max(0, original.split("\n").length - text.split("\n").length), note };
  }
  return compressKV(lines);
}
