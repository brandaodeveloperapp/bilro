import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { open, observe } from "../lib/learn.js";
import { evaluate, verdicts } from "../lib/ready.js";

const db = () => open(join(mkdtempSync(join(tmpdir(), "bilro-r-")), "l.db"));

test("banco vazio nao libera nada", () => {
  const m = evaluate(db());
  assert.equal(m.total, 0);
  for (const v of verdicts(m)) assert.ok(v.missing.length > 0, v.tool);
});

test("cobertura conta so comando com 3+ execucoes", () => {
  const d = db();
  for (let i = 0; i < 5; i++) observe(d, "npm test", `ok ${i}\nsempre igual`);
  observe(d, "npm run build", "rodou uma vez");
  const m = evaluate(d);
  assert.equal(m.total, 2);
  assert.equal(m.learned, 1);
});

test("perda de linha de falha reprova o portao do rtk", () => {
  const d = db();
  for (let i = 0; i < 5; i++) observe(d, "ci.sh", "FAIL auth\ntudo bem");
  const m = evaluate(d);
  assert.equal(m.lost.length, 0);
  assert.ok(!verdicts(m).find((v) => v.tool === "rtk").missing.includes("sinal"));
});
