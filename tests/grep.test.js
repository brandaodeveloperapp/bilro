import test from "node:test";
import assert from "node:assert/strict";
import { compress } from "../lib/grep.js";

test("conteudo do match nunca e cortado", () => {
  const raw = "a.ts:1:const token = process.env.SECRET\nb.ts:9:const token = process.env.SECRET";
  const r = compress(raw);
  assert.match(r.text, /const token = process\.env\.SECRET/);
  assert.equal(r.hits, 2);
});

test("match identico em varias linhas vira uma linha com contagem", () => {
  const raw = Array.from({ length: 20 }, (_, i) => `a.ts:${i + 1}:import x`).join("\n");
  const r = compress(raw);
  assert.equal(r.hits, 20);
  assert.ok(r.text.length < raw.length);
  assert.match(r.text, /\+17/);
});

test("arquivo com muitos trechos distintos diz quantos ficaram de fora", () => {
  const raw = Array.from({ length: 40 }, (_, i) => `a.ts:${i + 1}:linha unica ${i}`).join("\n");
  const r = compress(raw, { perFile: 5 });
  assert.match(r.text, /… 35 outros trechos/);
});

test("saida que nao e grep passa intacta", () => {
  const raw = "isso nao tem formato de match\nnem isso";
  assert.equal(compress(raw).text, raw);
});
