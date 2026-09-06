import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { detect, compress, name } from "../lib/shapes/diff.js";
import { MIN_CONFIDENCE } from "../lib/shapes/contract.js";

const fixture = (f) => readFileSync(join(import.meta.dirname, "fixtures", f), "utf8");

test("nome estavel", () => {
  assert.equal(name, "diff");
});

test("detecta git diff real (multi-arquivo) como diff", () => {
  const raw = fixture("git-diff-real.txt");
  assert.ok(detect(raw.split("\n")) >= MIN_CONFIDENCE);
});

test("detecta git show -p real (codigo, com contexto de verdade) como diff", () => {
  const raw = fixture("git-show-code.txt");
  assert.ok(detect(raw.split("\n")) >= MIN_CONFIDENCE);
});

test("detecta diff --cc de um merge conflict real como diff", () => {
  const raw = fixture("git-diff-conflict.txt");
  assert.ok(detect(raw.split("\n")) >= MIN_CONFIDENCE);
});

test("detecta git status real como diff (forma de status de versionamento)", () => {
  const raw = fixture("git-status-real.txt");
  assert.ok(detect(raw.split("\n")) >= MIN_CONFIDENCE);
});

test("detecta git status de um merge conflito real (unmerged paths)", () => {
  const raw = fixture("git-status-conflict.txt");
  assert.ok(detect(raw.split("\n")) >= MIN_CONFIDENCE);
});

test("NAO detecta tabela docker ps como diff (falso positivo cruzado)", () => {
  const raw = fixture("docker-ps.txt");
  assert.ok(detect(raw.split("\n")) < MIN_CONFIDENCE);
});

test("NAO detecta tabela docker images como diff", () => {
  const raw = fixture("docker-images.txt");
  assert.ok(detect(raw.split("\n")) < MIN_CONFIDENCE);
});

test("texto solto sem marcador de diff ou status confidence zero", () => {
  assert.equal(detect(["so um texto qualquer", "sem estrutura nenhuma"]), 0);
});

test("comprime git show -p real: corta contexto real, mantem todo +/- e headers de arquivo", () => {
  const raw = fixture("git-show-code.txt");
  const lines = raw.split("\n");
  const r = compress(lines);
  assert.ok(r.text.length < raw.length);
  const reduction = 1 - r.text.length / raw.length;
  assert.ok(reduction > 0.1, `esperava corte real > 10%, obteve ${(reduction * 100).toFixed(1)}%`);
  assert.ok(r.dropped > 0);

  for (const l of lines) {
    if (/^diff --git /.test(l) || /^[+-](?!\+\+|--)/.test(l)) {
      assert.ok(r.text.includes(l), `linha estrutural perdida: ${l}`);
    }
  }
});

test("diff grande de verdade estoura orcamento e avisa explicitamente, nunca corta calado", () => {
  const raw = fixture("git-diff-real.txt");
  const lines = raw.split("\n");
  const r = compress(lines);
  const totalFiles = lines.filter((l) => /^diff --git /.test(l)).length;
  assert.ok(totalFiles > 1);
  assert.match(r.note, /orcamento estourado/);
  assert.match(r.note, /arquivos omitidos/);

  const shownFiles = r.text.split("\n").filter((l) => /^diff --git /.test(l)).length;
  assert.ok(shownFiles < totalFiles, "deveria ter cortado pelo menos um arquivo");
  assert.ok(shownFiles > 0, "nunca deveria zerar tudo silenciosamente");
});

test("conflito de merge (diff --cc): marcadores <<<<<<< ======= >>>>>>> sobrevivem ao corte", () => {
  const raw = fixture("git-diff-conflict.txt");
  const r = compress(raw.split("\n"));
  assert.match(r.text, /<<<<<<< HEAD/);
  assert.match(r.text, /=======/);
  assert.match(r.text, />>>>>>> b2/);
});

test("conflito de merge (git status): both modified sobrevive por completo, nao vira so a contagem", () => {
  const raw = fixture("git-status-conflict.txt");
  const r = compress(raw.split("\n"));
  assert.match(r.text, /both modified:\s+f\.txt/);
});

test("git status real: agrupa por tipo com contagem e derruba so o boilerplate de dica", () => {
  const raw = fixture("git-status-real.txt");
  const lines = raw.split("\n");
  const r = compress(lines);
  assert.ok(r.text.length < raw.length);
  assert.doesNotMatch(r.text, /^\s+\(use "git/m);

  const modifiedCount = lines.filter((l) => l.trim().startsWith("modified:")).length;
  assert.match(r.text, new RegExp(`modified \\(${modifiedCount}\\):`));

  for (const l of lines) {
    const m = /^\t(?:modified|deleted|new file|renamed):\s*(.+)$/.exec(l);
    if (m) assert.ok(r.text.includes(m[1].trim()), `arquivo sumiu do agrupamento: ${m[1]}`);
  }
});

test("linha de contexto com isSevere (erro) nunca e cortada mesmo dentro de um hunk", () => {
  const lines = [
    "diff --git a/x.txt b/x.txt",
    "index 111..222 100644",
    "--- a/x.txt",
    "+++ b/x.txt",
    "@@ -1,6 +1,6 @@",
    " linha de contexto comum",
    " linha de contexto comum tambem",
    " connection refused ao conectar no banco",
    "-valor antigo",
    "+valor novo",
    " mais uma linha de contexto sem importancia",
  ];
  const r = compress(lines);
  assert.ok(r.text.includes(" connection refused ao conectar no banco"));
});

test("shape nao muda nem reordena as linhas +/- e de header", () => {
  const raw = fixture("git-show-code.txt");
  const lines = raw.split("\n");
  const r = compress(lines);
  const changeLinesIn = lines.filter((l) => /^diff --git /.test(l) || /^[+-](?!\+\+|--)/.test(l));
  const changeLinesOut = r.text.split("\n").filter((l) => /^diff --git /.test(l) || /^[+-](?!\+\+|--)/.test(l));
  assert.deepEqual(changeLinesOut, changeLinesIn);
});

test("sem estrutura de diff ou status, compress devolve entrada intacta", () => {
  const lines = ["texto qualquer", "sem hunk sem status"];
  const r = compress(lines);
  assert.equal(r.text, lines.join("\n"));
  assert.equal(r.dropped, 0);
  assert.equal(r.note, "");
});
