import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { name, detect, compress } from "../lib/shapes/install-log.js";
import { detect as detectListing } from "../lib/shapes/listing.js";
import { detect as detectKeyvalue } from "../lib/shapes/keyvalue.js";

const FIX = join(import.meta.dirname, "fixtures");
const load = (f) => readFileSync(join(FIX, f), "utf8");
const linesOf = (text) => text.split("\n");

test("nome estavel", () => {
  assert.equal(name, "install-log");
});

test("detecta npm install real com alta confianca", () => {
  const text = load("install-npm-real.txt");
  assert.ok(detect(linesOf(text)) >= 0.6);
});

test("detecta pip install real com alta confianca", () => {
  const text = load("install-pip-real.txt");
  assert.ok(detect(linesOf(text)) >= 0.6);
});

test("detecta uv add real com alta confianca", () => {
  const text = load("install-uv-real.txt");
  assert.ok(detect(linesOf(text)) >= 0.6);
});

test("nao detecta listagem de arquivo como install-log", () => {
  const text = load("listing-find.txt");
  assert.ok(detect(linesOf(text)) < 0.6);
  const grepText = load("listing-grep.txt");
  assert.ok(detect(linesOf(grepText)) < 0.6);
});

test("nao detecta JSON/keyvalue como install-log", () => {
  const text = load("keyvalue-curl-real.json");
  assert.ok(detect(linesOf(text)) < 0.6);
});

test("nao detecta diff de git como install-log", () => {
  const text = load("git-diff-real.txt");
  assert.ok(detect(linesOf(text)) < 0.6);
});

test("listing e keyvalue nao se confundem com install-log no sentido inverso", () => {
  const text = load("install-npm-real.txt");
  assert.ok(detectListing(linesOf(text)) < 0.6);
  assert.ok(detectKeyvalue(linesOf(text)) < 0.6);
});

test("compress de npm install real mantem sumario e todo warning, corta progresso e funding", () => {
  const text = load("install-npm-real.txt");
  const before = text.length;
  const r = compress(linesOf(text));
  assert.ok(r.text.length < before, `esperava reducao: antes=${before} depois=${r.text.length}`);
  assert.ok(r.text.includes("added 126 packages"));
  assert.ok(r.text.includes("found 0 vulnerabilities"));
  for (const dep of ["inflight@1.0.6", "rimraf@3.0.2", "glob@7.2.3"]) {
    assert.ok(r.text.includes(dep), `warning de ${dep} nao pode ser cortado`);
  }
  assert.ok(!r.text.includes("looking for funding"));
  assert.ok(!r.text.includes("run \`npm fund\`"));
});

test("compress de pip install real mantem Successfully installed e corta Collecting/Downloading", () => {
  const text = load("install-pip-real.txt");
  const before = text.length;
  const r = compress(linesOf(text));
  const cut = 1 - r.text.length / before;
  assert.ok(cut > 0.3, `esperava corte razoavel, obteve ${(cut * 100).toFixed(1)}%`);
  assert.ok(r.text.includes("Successfully installed"));
  assert.ok(!r.text.includes("Collecting requests"));
  assert.ok(!r.text.includes("Downloading requests"));
});

test("compress de uv add real mantem contagem de pacotes e corta lista individual resolvida", () => {
  const text = load("install-uv-real.txt");
  const r = compress(linesOf(text));
  assert.ok(r.text.includes("Installed 12 packages in 9ms"));
  assert.ok(!r.text.includes("+ blinker==1.9.0"));
  assert.ok(r.dropped >= 10);
});

test("barra de progresso com retorno de carro (\\r) nunca sobrevive como lixo", () => {
  const lines = [
    "Collecting bigpkg",
    "Downloading bigpkg-1.0.0.whl (900 MB)\r 10%|#         | 90/900 MB\r 55%|#####     | 495/900 MB\r 100%|##########| 900/900 MB",
    "Successfully installed bigpkg-1.0.0",
  ];
  const r = compress(lines);
  assert.ok(!r.text.includes("\r"));
  assert.ok(!r.text.includes("10%|"));
  assert.ok(r.text.includes("Successfully installed bigpkg-1.0.0"));
});

test("falha de peer dependency e de audit nunca sao cortadas mesmo em meio a enxurrada", () => {
  const flood = [];
  for (let i = 0; i < 40; i++) flood.push(`npm verbose fetch manifest pkg${i}@1.0.0`);
  const lines = [
    ...flood,
    "npm error ERESOLVE unable to resolve dependency tree",
    "npm error peer react@\">=18\" from some-lib@2.0.0",
    "found 3 high severity vulnerabilities, run `npm audit fix`",
    "added 40 packages in 2s",
  ];
  const r = compress(lines);
  assert.ok(r.text.includes("ERESOLVE unable to resolve dependency tree"));
  assert.ok(r.text.includes("3 high severity vulnerabilities"));
  assert.ok(r.dropped >= 30);
});

test("compress retorna string mesmo sem nenhum sumario reconhecido (fallback seguro)", () => {
  const lines = ["algo qualquer", "outra linha qualquer"];
  const r = compress(lines);
  assert.equal(typeof r.text, "string");
});
