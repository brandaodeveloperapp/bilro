import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { linksOf, lint } from "../lib/graph.js";

function vault(files) {
  const dir = mkdtempSync(join(tmpdir(), "bilro-g-"));
  for (const [name, body] of Object.entries(files))
    writeFileSync(join(dir, name), `---\nname: ${name.replace(/\.md$/, "")}\n---\n${body}`);
  return dir;
}

test("le as quatro formas de link", () => {
  const dir = vault({ "a.md": "ver [[b]], [[c|apelido]], [[d#secao]] e [[e#s|x]]" });
  const found = linksOf({ body: "ver [[b]], [[c|apelido]], [[d#secao]] e [[e#s|x]]" });
  assert.deepEqual(found.sort(), ["b", "c", "d", "e"]);
});

test("acusa link para memoria inexistente", () => {
  const dir = vault({ "a.md": "aponta pra [[sumida]]" });
  const r = lint(dir);
  assert.equal(r.broken.length, 1);
  assert.equal(r.broken[0].to, "sumida");
});

test("link valido nao vira quebrado", () => {
  const dir = vault({ "a.md": "vai pra [[b]]", "b.md": "fim" });
  assert.equal(lint(dir).broken.length, 0);
});

test("memoria sem link entrando nem saindo e orfa", () => {
  const dir = vault({ "a.md": "vai pra [[b]]", "b.md": "fim", "sozinha.md": "ninguem me cita" });
  assert.deepEqual(lint(dir).orphans, ["sozinha"]);
});

test("memoria muito citada aparece como hub", () => {
  const files = { "centro.md": "sou o centro" };
  for (let i = 0; i < 5; i++) files[`n${i}.md`] = "olha [[centro]]";
  const r = lint(vault(files));
  assert.equal(r.hubs[0].name, "centro");
  assert.equal(r.hubs[0].incoming, 5);
});
