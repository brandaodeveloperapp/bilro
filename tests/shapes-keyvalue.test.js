import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { name, detect, compress } from "../lib/shapes/keyvalue.js";
import { detect as detectListing } from "../lib/shapes/listing.js";
import { detect as detectInstall } from "../lib/shapes/install-log.js";

const FIX = join(import.meta.dirname, "fixtures");
const load = (f) => readFileSync(join(FIX, f), "utf8");
const linesOf = (text) => text.split("\n");

test("nome estavel", () => {
  assert.equal(name, "keyvalue");
});

test("detecta JSON real (curl github releases) com confianca maxima", () => {
  const text = load("keyvalue-curl-real.json");
  assert.equal(detect(linesOf(text)), 1);
});

test("detecta JSON real (node process.versions) com confianca maxima", () => {
  const text = load("keyvalue-versions-real.json");
  assert.equal(detect(linesOf(text)), 1);
});

test("detecta env real KEY=value com alta confianca", () => {
  const text = load("keyvalue-env-real.txt");
  assert.ok(detect(linesOf(text)) >= 0.6);
});

test("nao detecta listagem de arquivo como keyvalue", () => {
  const text = load("listing-find.txt");
  assert.ok(detect(linesOf(text)) < 0.6);
  const grepText = load("listing-grep.txt");
  assert.ok(detect(linesOf(grepText)) < 0.6);
});

test("nao detecta install-log como keyvalue", () => {
  const text = load("install-npm-real.txt");
  assert.ok(detect(linesOf(text)) < 0.6);
  const pipText = load("install-pip-real.txt");
  assert.ok(detect(linesOf(pipText)) < 0.6);
});

test("nao detecta diff de git como keyvalue", () => {
  const text = load("git-diff-real.txt");
  assert.ok(detect(linesOf(text)) < 0.6);
});

test("listing e install-log nao se confundem com keyvalue no sentido inverso", () => {
  const text = load("keyvalue-versions-real.json");
  assert.ok(detectListing(linesOf(text)) < 0.6);
  assert.ok(detectInstall(linesOf(text)) < 0.6);
});

test("compress de JSON real colapsa array homogeneo longo e trunca string longa, reduzindo muito os bytes", () => {
  const text = load("keyvalue-curl-real.json");
  const before = text.length;
  const r = compress(linesOf(text));
  const cut = 1 - r.text.length / before;
  assert.ok(cut > 0.9, `esperava corte > 90%, obteve ${(cut * 100).toFixed(1)}%`);
  const parsed = JSON.parse(r.text);
  assert.ok(parsed["(resumo)"].includes("5 itens"));
  assert.ok(parsed["(exemplo)"].tag_name === "v26.8.1");
  assert.ok(parsed["(exemplo)"].body.includes("cortado"));
  assert.ok(r.note.includes("truncada"));
});

test("compress de JSON simples (process.versions) preserva todas as chaves, sem colapso de array", () => {
  const text = load("keyvalue-versions-real.json");
  const parsedBefore = JSON.parse(text);
  const r = compress(linesOf(text));
  const parsedAfter = JSON.parse(r.text);
  assert.deepEqual(Object.keys(parsedAfter).sort(), Object.keys(parsedBefore).sort());
  assert.equal(parsedAfter.node, parsedBefore.node);
});

test("compress de env real trunca PATH gigante mas preserva todas as chaves", () => {
  const text = load("keyvalue-env-real.txt");
  const keysBefore = linesOf(text)
    .filter((l) => l.includes("="))
    .map((l) => l.split("=")[0]);
  const r = compress(linesOf(text));
  for (const key of keysBefore) assert.ok(r.text.includes(`${key}=`), `chave ${key} sumiu`);
  assert.ok(r.text.length < text.length);
});

test("SEGURANCA: chave que parece segredo tem o valor mascarado em formato KEY=value", () => {
  const synthetic = [
    "DATABASE_URL=postgres://user:pass@host:5432/db",
    "API_KEY=sk_live_abcdefghijklmnopqrstuvwxyz123456",
    "AUTH_TOKEN=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.fake.sig",
    "PASSWORD=hunter2superSecret",
    "SENHA=trocarDepois123",
    "AUTHORIZATION=Bearer abc.def.ghi",
    "HOME=/Users/igorbrandao",
  ].join("\n");
  const r = compress(linesOf(synthetic));
  assert.ok(!r.text.includes("sk_live_abcdefghijklmnopqrstuvwxyz123456"));
  assert.ok(!r.text.includes("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.fake.sig"));
  assert.ok(!r.text.includes("hunter2superSecret"));
  assert.ok(!r.text.includes("trocarDepois123"));
  assert.ok(!r.text.includes("Bearer abc.def.ghi"));
  assert.ok(r.text.includes("HOME=/Users/igorbrandao"), "chave nao-secreta deve sobreviver intacta");
  assert.ok(r.text.includes("API_KEY=***MASCARADO***"));
  assert.ok(r.note.includes("mascarado"));
});

test("SEGURANCA: chave que parece segredo tem o valor mascarado em JSON aninhado", () => {
  const synthetic = JSON.stringify({
    service: "billing",
    config: {
      apiKey: "sk_live_shouldnotleak",
      nested: { password: "hunter2", bearerToken: "abc123xyz" },
    },
    port: 8080,
  });
  const r = compress(linesOf(synthetic));
  assert.ok(!r.text.includes("sk_live_shouldnotleak"));
  assert.ok(!r.text.includes("hunter2"));
  assert.ok(!r.text.includes("abc123xyz"));
  const parsed = JSON.parse(r.text);
  assert.equal(parsed.config.apiKey, "***MASCARADO***");
  assert.equal(parsed.config.nested.password, "***MASCARADO***");
  assert.equal(parsed.config.nested.bearerToken, "***MASCARADO***");
  assert.equal(parsed.port, 8080);
  assert.equal(parsed.service, "billing");
});

test("linha severa nao tem o valor truncado, so mascarado quando aplicavel", () => {
  const bigError = "x".repeat(400);
  const line = `LAST_ERROR=failed to connect: ${bigError}`;
  const r = compress(linesOf(line));
  assert.ok(r.text.includes(bigError), "erro grande nao deveria ser truncado");
});

test("compress retorna string valida para uma unica linha key=value simples", () => {
  const r = compress(["FOO=bar"]);
  assert.equal(r.text, "FOO=bar");
  assert.equal(r.dropped, 0);
});

test("senha embutida em string de conexao nunca chega ao contexto", () => {
  const linhas = [
    "DATABASE_URL=postgres://user:s3nh4Sup3rS3cr3t@db:5432/prod",
    "REDIS_URL=redis://:mypassword@cache:6379",
    "MONGO=mongodb://admin:Adm1nPass@mongo:27017/db",
    "AMQP=amqp://guest:guestpw@rabbit:5672",
  ];
  const out = compress(linhas).text;
  for (const segredo of ["s3nh4Sup3rS3cr3t", "mypassword", "Adm1nPass", "guestpw"])
    assert.doesNotMatch(out, new RegExp(segredo), `vazou ${segredo}`);
  assert.match(out, /db:5432\/prod/);
  assert.match(out, /cache:6379/);
});

test("chave que so contem a palavra key por acaso nao e mascarada", () => {
  const out = compress(["primaryKey=id", "monkeyName=george", "keyboard_layout=abnt2", "publicKeyPath=/etc/x.pub"]).text;
  assert.match(out, /primaryKey=id/);
  assert.match(out, /monkeyName=george/);
  assert.match(out, /keyboard_layout=abnt2/);
  assert.match(out, /publicKeyPath=\/etc\/x\.pub/);
});

test("url sem credencial passa intacta", () => {
  const out = compress(["OK=https://example.com/path", "GIT=git@github.com:user/repo.git"]).text;
  assert.match(out, /https:\/\/example\.com\/path/);
  assert.match(out, /git@github\.com:user\/repo\.git/);
});
