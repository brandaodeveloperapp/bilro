import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { signature, open, observe, denoise, novelty, similarity, rankByInformation } from "../lib/learn.js";

const db = () => open(join(mkdtempSync(join(tmpdir(), "bilro-l-")), "l.db"));

test("assinatura ignora numero e hash, nao o programa", () => {
  assert.equal(signature("git log abc1234def"), signature("git log 99fedcba111"));
  assert.equal(signature("jest a.test.js"), signature("jest a.test.js"));
  assert.notEqual(signature("npx jest"), signature("npx tsc"));
});

test("sem historico suficiente nao corta nada", () => {
  const d = db();
  const out = "linha fixa\noutra";
  observe(d, "cmd", out);
  const r = denoise(d, "cmd", out);
  assert.equal(r.learned, false);
  assert.equal(r.text, out);
});

test("linha presente em toda execucao vira ruido", () => {
  const d = db();
  const fixo = "cabecalho\nrodape";
  for (let i = 0; i < 4; i++) observe(d, "cmd", fixo);
  const r = denoise(d, "cmd", fixo);
  assert.equal(r.learned, true);
  assert.equal(r.text, "");
  assert.equal(r.dropped, 2);
});

test("linha nova SEMPRE sobrevive ao corte", () => {
  const d = db();
  const fixo = "cabecalho\nrodape";
  for (let i = 0; i < 5; i++) observe(d, "cmd", fixo);
  const r = denoise(d, "cmd", `${fixo}\nFALHA: teste quebrou`);
  assert.match(r.text, /FALHA: teste quebrou/);
});

test("saida identica a anterior e sinalizada", () => {
  const d = db();
  observe(d, "cmd", "igual");
  const segunda = observe(d, "cmd", "igual");
  assert.equal(segunda.unchanged, true);
  assert.equal(observe(d, "cmd", "diferente").unchanged, false);
});

test("novidade lista so o que nao existia antes", () => {
  assert.deepEqual(novelty("a\nb", "a\nb\nc"), ["c"]);
  assert.deepEqual(novelty("a\nb", "a\nb"), []);
});

test("numero variavel nao conta como linha nova", () => {
  const d = db();
  for (let i = 0; i < 4; i++) observe(d, "cmd", `rodou em ${i * 10}ms`);
  const r = denoise(d, "cmd", "rodou em 999ms");
  assert.equal(r.text, "");
});

test("similaridade vai de 1 a 0 conforme a saida muda", () => {
  const base = ["compilando modulo", "rodando testes", "verificando tipos", "gerando bundle"].join("\n");
  assert.equal(similarity(base, base), 1);
  assert.ok(similarity(base, base + "\nERRO no bundle") > 0.7);
  assert.equal(similarity(base, "deploy iniciado\npods prontos"), 0);
});

test("corte por informacao guarda a linha rara, nao a primeira", () => {
  const d = db();
  const rotina = Array.from({ length: 200 }, (_, i) => `passo ${String.fromCharCode(97 + (i % 26))}`).join("\n");
  for (let i = 0; i < 4; i++) observe(d, "cmd", rotina);
  const r = rankByInformation(d, "cmd", `${rotina}\nFALHA rarissima aqui`, 10);
  assert.equal(r.ranked, true);
  assert.match(r.text, /FALHA rarissima aqui/);
});

test("saida curta nao e cortada por informacao", () => {
  const d = db();
  const r = rankByInformation(d, "cmd", "a\nb\nc", 80);
  assert.equal(r.ranked, false);
  assert.equal(r.text, "a\nb\nc");
});

test("ordem original e preservada no corte", () => {
  const d = db();
  const r = rankByInformation(d, "novo", "zebra\nabelha\ncachorro\ndragao", 3);
  const idx = ["zebra", "abelha", "cachorro", "dragao"].filter((w) => r.text.includes(w));
  assert.deepEqual(
    idx,
    r.text.split("\n").map((l) => l.trim()),
  );
});
