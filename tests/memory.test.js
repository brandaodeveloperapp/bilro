import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { loadMemories, claimsIn, audit, ageInDays } from "../lib/memory.js";

function vault(files) {
  const dir = mkdtempSync(join(tmpdir(), "bilro-"));
  for (const [name, body] of Object.entries(files)) writeFileSync(join(dir, name), body);
  return dir;
}

const fm = (extra = "") => `---\nname: teste\ntype: project\nmodified: 2026-08-01T00:00:00.000Z\n${extra}---\n`;

test("ignora o indice e os backups", () => {
  const dir = vault({ "MEMORY.md": "x", "a.md.bak-1": "y", "boa.md": fm() + "corpo" });
  assert.equal(loadMemories(dir).length, 1);
});

test("detecta versao, dinheiro e contagem como afirmacao", () => {
  const dir = vault({ "a.md": fm() + "subiu 1.1.4 e custou R$750 com 36 referrals" });
  const kinds = claimsIn(loadMemories(dir)[0]).map((c) => c.kind);
  assert.deepEqual(kinds.sort(), ["contagem", "valor", "versao"]);
});

test("texto sem fato que envelhece nao vira suspeita", () => {
  const dir = vault({ "a.md": fm() + "sempre rodar o teste antes de subir" });
  assert.equal(audit(loadMemories(dir)).length, 0);
});

test("memoria com verify: e verificavel, nao suspeita", () => {
  const dir = vault({ "a.md": fm("verify: echo 1.1.8\nexpect: 1.1.8\n") + "versao 1.1.8" });
  const [row] = audit(loadMemories(dir), { now: Date.parse("2026-09-06T00:00:00Z") });
  assert.equal(row.status, "verificavel");
});

test("afirmacao velha sem verify vira suspeita", () => {
  const dir = vault({ "a.md": fm() + "a versao e 1.1.4" });
  const [row] = audit(loadMemories(dir), { now: Date.parse("2026-09-06T00:00:00Z") });
  assert.equal(row.status, "suspeita");
});

test("afirmacao recente ainda nao e suspeita", () => {
  const dir = vault({ "a.md": fm() + "a versao e 1.1.4" });
  const [row] = audit(loadMemories(dir), { now: Date.parse("2026-08-03T00:00:00Z") });
  assert.equal(row.status, "afirma");
});

test("idade em dias sai do frontmatter", () => {
  const dir = vault({ "a.md": fm() + "1.0.0" });
  assert.equal(ageInDays(loadMemories(dir)[0], Date.parse("2026-08-11T00:00:00Z")), 10);
});
