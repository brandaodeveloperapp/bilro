import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { detect, compress, name } from "../lib/shapes/table.js";
import { MIN_CONFIDENCE } from "../lib/shapes/contract.js";

const fixture = (f) => readFileSync(join(import.meta.dirname, "fixtures", f), "utf8");

test("nome estavel", () => {
  assert.equal(name, "table");
});

test("detecta docker ps -a real como tabela", () => {
  const raw = fixture("docker-ps.txt");
  assert.ok(detect(raw.split("\n")) >= MIN_CONFIDENCE);
});

test("detecta docker ps com container Up e Exited misturados", () => {
  const raw = fixture("docker-ps-mixed.txt");
  assert.ok(detect(raw.split("\n")) >= MIN_CONFIDENCE);
});

test("detecta docker images real, ignorando linha de warning antes do header", () => {
  const raw = fixture("docker-images.txt");
  assert.ok(detect(raw.split("\n")) >= MIN_CONFIDENCE);
});

test("NAO detecta diff unificado como tabela (falso positivo cruzado)", () => {
  const raw = fixture("git-diff-real.txt");
  assert.ok(detect(raw.split("\n")) < MIN_CONFIDENCE);
});

test("NAO detecta git status como tabela", () => {
  const raw = fixture("git-status-real.txt");
  assert.ok(detect(raw.split("\n")) < MIN_CONFIDENCE);
});

test("NAO detecta git log oneline como tabela", () => {
  const raw = fixture("git-log.txt");
  assert.ok(detect(raw.split("\n")) < MIN_CONFIDENCE);
});

test("texto sem header nao e tabela", () => {
  assert.equal(detect(["algum texto qualquer", "outra linha solta"]), 0);
});

test("comprime docker images real: remove coluna EXTRA constante e colapsa padding", () => {
  const raw = fixture("docker-images.txt");
  const r = compress(raw.split("\n"));
  assert.ok(r.text.length < raw.length);
  const reduction = 1 - r.text.length / raw.length;
  assert.ok(reduction > 0.3, `esperava corte real > 30%, obteve ${(reduction * 100).toFixed(1)}%`);
  assert.match(r.note, /EXTRA=U/);
  assert.equal((r.text.match(/\bEXTRA\b/g) || []).length, 1, "coluna constante deve ser nomeada uma vez, nunca repetida por linha");
  for (const line of r.text.split("\n").slice(2)) assert.doesNotMatch(line, /\bEXTRA\b/);
});

test("comprime docker ps misturado: linha Up encolhe, linhas Exited sobrevivem inteiras", () => {
  const raw = fixture("docker-ps-mixed.txt");
  const lines = raw.split("\n");
  const r = compress(lines);
  assert.ok(r.text.length < raw.length);

  const exitedLines = lines.filter((l) => l.includes("Exited"));
  for (const original of exitedLines) {
    assert.ok(r.text.includes(original), `linha unhealthy foi alterada: ${original}`);
  }

  const upLineOut = r.text.split("\n").find((l) => l.includes("bilro-fixture-tmp"));
  assert.ok(upLineOut, "linha do container saudavel sumiu");
  const upLineIn = lines.find((l) => l.includes("bilro-fixture-tmp"));
  assert.ok(upLineOut.length < upLineIn.length, "linha saudavel deveria ter encolhido");
});

test("linha isSevere sobrevive mesmo sem coluna de status reconhecida", () => {
  const lines = [
    "NAME  VALUE",
    "a     ok",
    "b     connection refused",
    "c     ok",
  ];
  const r = compress(lines);
  assert.ok(r.text.includes("connection refused"));
  assert.ok(r.text.split("\n").some((l) => l === lines[2]));
});

test("linha com status nao saudavel (nao Running/Ready/Active/Up) sobrevive inteira mesmo sem isSevere", () => {
  const lines = [
    "NAME  STATUS",
    "pod-a Running",
    "pod-b CrashLoopBackOff",
    "pod-c Running",
  ];
  const r = compress(lines);
  assert.ok(r.text.split("\n").includes("pod-b CrashLoopBackOff"));
});

test("sem header reconhecivel, compress devolve entrada intacta", () => {
  const lines = ["so texto", "mais texto", "linha final"];
  const r = compress(lines);
  assert.equal(r.text, lines.join("\n"));
  assert.equal(r.dropped, 0);
  assert.equal(r.note, "");
});

test("ordem das linhas sobreviventes nunca e alterada", () => {
  const raw = fixture("docker-ps-mixed.txt");
  const lines = raw.split("\n");
  const r = compress(lines);
  const namesIn = lines.slice(1).filter((l) => l.trim()).map((l) => l.trim().split(/\s+/).pop());
  const namesOut = r.text.split("\n").slice(1).filter((l) => l.trim()).map((l) => l.trim().split(/\s+/).pop());
  assert.deepEqual(namesOut, namesIn);
});
