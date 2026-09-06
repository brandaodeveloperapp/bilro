import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { open, observe } from "../lib/learn.js";
import { proposals, draft } from "../lib/propose.js";

const db = () => open(join(mkdtempSync(join(tmpdir(), "bilro-p-")), "p.db"));

test("comando pouco usado nao vira proposta", () => {
  const d = db();
  observe(d, "npx jest a", "tudo ok");
  assert.equal(proposals(d).length, 0);
});

test("comando repetido vira receita", () => {
  const d = db();
  for (let i = 0; i < 6; i++) observe(d, "npx jest a", "tudo ok");
  const r = proposals(d);
  assert.equal(r[0].kind, "receita");
  assert.equal(r[0].seen, 6);
});

test("linha de falha recorrente vira armadilha", () => {
  const d = db();
  for (let i = 0; i < 3; i++) observe(d, "deploy prod", "subindo\nError: connection refused pelo cluster remoto");
  const r = proposals(d);
  assert.ok(r.some((p) => p.kind === "armadilha" && /connection refused/.test(p.subject)));
});

test("saida saudavel nao vira armadilha", () => {
  const d = db();
  for (let i = 0; i < 6; i++) observe(d, "build ok", "compilado com sucesso\ntudo certo por aqui");
  assert.equal(proposals(d).filter((p) => p.kind === "armadilha").length, 0);
});

test("rascunho sai com frontmatter valido e slug utilizavel", () => {
  const d = draft({ kind: "armadilha", subject: "Erro: conexão RECUSADA no cluster", seen: 3, why: "x" });
  assert.match(d.slug, /^[a-z0-9-]+$/);
  assert.match(d.content, /^---\nname: /);
  assert.match(d.content, /type: project/);
});
