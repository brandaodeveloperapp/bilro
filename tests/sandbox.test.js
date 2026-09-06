import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { chunk, toMatchQuery, open, index, search, run } from "../lib/sandbox.js";

function db() {
  return open(join(mkdtempSync(join(tmpdir(), "bilro-db-")), "i.db"));
}

test("quebra por linha em branco e mantem o bloco inteiro", () => {
  assert.deepEqual(chunk("um\ndois\n\ntres"), ["um\ndois", "tres"]);
});

test("bloco gigante e cortado para nao virar um chunk so", () => {
  const grande = Array.from({ length: 130 }, (_, i) => `l${i}`).join("\n");
  assert.equal(chunk(grande, 60).length, 3);
});

test("pontuacao de caminho nao quebra o FTS5", () => {
  assert.equal(toMatchQuery("features/references"), '"features/references"');
  assert.equal(toMatchQuery("  "), null);
});

test("aspas do usuario nao viram injecao de sintaxe", () => {
  assert.equal(toMatchQuery('a" OR "b'), '"a" OR "OR" OR "b"');
});

test("indexa e recupera pelo termo", () => {
  const d = db();
  index(d, { label: "l", body: "spinner preso no query store\n\ncodigo de afiliado sem prazo", source: "s" });
  assert.equal(search(d, "afiliado").length, 1);
  assert.equal(search(d, "inexistente").length, 0);
});

test("run devolve o tamanho retido, nao o conteudo", () => {
  const r = run({ command: "printf 'linha\\n%.0s' $(seq 1 200)", queries: [] });
  assert.equal(r.failed, false);
  assert.ok(r.withheldTokens > 100);
  assert.ok(r.chunks >= 1);
});

test("comando que falha nao explode e ainda indexa a saida", () => {
  const r = run({ command: "echo antes; exit 3", queries: [] });
  assert.equal(r.failed, true);
  assert.ok(r.withheldTokens > 0);
});
