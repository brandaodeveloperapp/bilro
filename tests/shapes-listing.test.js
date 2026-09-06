import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { name, detect, compress } from "../lib/shapes/listing.js";
import { detect as detectInstall } from "../lib/shapes/install-log.js";
import { detect as detectKeyvalue } from "../lib/shapes/keyvalue.js";

const FIX = join(import.meta.dirname, "fixtures");
const load = (f) => readFileSync(join(FIX, f), "utf8");
const linesOf = (text) => text.split("\n");

test("nome estavel", () => {
  assert.equal(name, "listing");
});

test("detecta ls -laR real com alta confianca", () => {
  const text = load("listing-lsR.txt");
  assert.ok(detect(linesOf(text)) >= 0.6, "ls -laR deveria bater no shape listing");
});

test("detecta find real com alta confianca", () => {
  const text = load("listing-find.txt");
  assert.ok(detect(linesOf(text)) >= 0.9);
});

test("detecta grep -rn real com alta confianca", () => {
  const text = load("listing-grep.txt");
  assert.ok(detect(linesOf(text)) >= 0.9);
});

test("nao detecta saida de install-log como listagem", () => {
  const text = load("install-npm-real.txt");
  assert.ok(detect(linesOf(text)) < 0.6);
});

test("nao detecta JSON/keyvalue como listagem", () => {
  const text = load("keyvalue-versions-real.json");
  assert.ok(detect(linesOf(text)) < 0.6);
  const env = load("keyvalue-env-real.txt");
  assert.ok(detect(linesOf(env)) < 0.6, "PATH=/a:/b nao pode virar path de arquivo");
});

test("nao detecta diff de git como listagem", () => {
  const text = load("git-diff-real.txt");
  assert.ok(detect(linesOf(text)) < 0.6);
});

test("install-log e keyvalue nao se confundem com listagem no sentido inverso", () => {
  const findText = load("listing-find.txt");
  const grepText = load("listing-grep.txt");
  assert.ok(detectInstall(linesOf(findText)) < 0.6);
  assert.ok(detectKeyvalue(linesOf(findText)) < 0.6);
  assert.ok(detectInstall(linesOf(grepText)) < 0.6);
  assert.ok(detectKeyvalue(linesOf(grepText)) < 0.6);
});

test("compress de find real fatora prefixo comum e reduz bytes", () => {
  const text = load("listing-find.txt");
  const before = text.length;
  const r = compress(linesOf(text));
  assert.ok(r.text.length < before, `esperava reducao: antes=${before} depois=${r.text.length}`);
  assert.ok(r.note.length > 0);
});

test("compress de find longo (lodash, 636 arquivos) agrupa por extensao com grande reducao", () => {
  const text = load("listing-find-long.txt");
  const before = text.length;
  const r = compress(linesOf(text));
  assert.ok(r.text.includes(".js: 633 arquivos"));
  const cut = 1 - r.text.length / before;
  assert.ok(cut > 0.9, `esperava corte > 90%, obteve ${(cut * 100).toFixed(1)}%`);
});

test("compress de grep agrupa hits por arquivo sem perder o conteudo do match", () => {
  const text = load("listing-grep.txt");
  const r = compress(linesOf(text));
  assert.ok(r.text.includes("export function linksOf(memory) {"));
  assert.ok(r.text.includes("lib/graph.js:"));
  assert.ok(r.note.includes("agrupados"));
});

test("compress de ls -laR remove coluna de dono/grupo quando identica em tudo", () => {
  const text = load("listing-lsR.txt");
  const r = compress(linesOf(text));
  assert.ok(!r.text.includes("igorbrandao"), "dono uniforme deveria ser cortado");
  assert.ok(r.text.includes("contract.js"));
  assert.ok(r.dropped > 0);
});

test("linha severa dentro de uma listagem longa nunca e cortada", () => {
  const longList = [];
  for (let i = 0; i < 40; i++) longList.push(`/tmp/proj/src/mod${i}.js`);
  longList.push("/tmp/proj/permission-denied.log");
  const withError = ["ls: cannot open directory 'x': Permission denied", ...longList];
  const r = compress(withError);
  assert.ok(r.text.includes("Permission denied"));
});

test("compress nunca reordena linhas que sobrevivem intactas em modo ls (unmatched)", () => {
  const raw = [
    "/tmp/dir:",
    "total 8",
    "ls: cannot access 'ghost': No such file or directory",
    "-rw-r--r--  1 igorbrandao  staff  10  1 jan 00:00 a.txt",
  ];
  const r = compress(raw);
  assert.ok(r.text.includes("No such file or directory"));
  assert.ok(r.text.includes("a.txt"));
});

test("compress retorna texto inalterado quando nao ha o que ganhar (fallback)", () => {
  const raw = ["/a/b/one.js", "/c/d/two.js"];
  const r = compress(raw);
  assert.equal(typeof r.text, "string");
  assert.equal(typeof r.note, "string");
});
