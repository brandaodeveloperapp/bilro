/**
 * A shape is a way output is structured, not a tool that produced it. Dozens
 * of programs emit the same columnar table or the same file:line diagnostic
 * list, so compressing by shape covers tools nobody wrote a rule for.
 *
 * Every shape module exports:
 *   name     stable identifier
 *   detect(lines) -> 0..1   confidence this output has that shape
 *   compress(lines) -> { text, dropped, note }
 *
 * Rules every shape must obey:
 *   - never drop a line isSevere() calls a failure
 *   - never reorder surviving lines
 *   - compress returns the input unchanged when it cannot do better
 */
export const MIN_CONFIDENCE = 0.6;

export function pick(shapes, lines) {
  let best = null;
  for (const s of shapes) {
    const c = s.detect(lines);
    if (c >= MIN_CONFIDENCE && (!best || c > best.confidence)) best = { shape: s, confidence: c };
  }
  return best;
}

export function apply(shapes, text) {
  const lines = text.split("\n");
  const hit = pick(shapes, lines);
  if (!hit) return { text, shape: null, dropped: 0 };
  const r = hit.shape.compress(lines);
  if (r.text.length >= text.length) return { text, shape: null, dropped: 0 };
  return { ...r, shape: hit.shape.name, confidence: hit.confidence };
}
