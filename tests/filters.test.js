import { test } from "node:test";
import assert from "node:assert/strict";
import { filter, ruleFor, collapseRepeats } from "../lib/filters.js";

test("escolhe a regra pelo comando", () => {
  assert.equal(ruleFor("npx jest src/").name, "jest");
  assert.equal(ruleFor("git status").name, "git-status");
  assert.equal(ruleFor("echo oi"), null);
});

test("corta stack de node_modules mas mantem a falha", () => {
  const out = ["● teste falhou", "    at Object.<anonymous>", "  node_modules/jest/x.js:1", "Expected: 3"].join("\n");
  const r = filter("npx jest", out);
  assert.match(r.text, /teste falhou/);
  assert.match(r.text, /Expected: 3/);
  assert.doesNotMatch(r.text, /node_modules/);
});

test("colapsa linha repetida com contagem", () => {
  assert.deepEqual(collapseRepeats(["a", "a", "a", "b"]), ["a   (×3)", "b"]);
});

test("nunca devolve vazio quando havia saida", () => {
  const r = filter("npx jest", "  \n  \n");
  assert.equal(typeof r.text, "string");
});

test("comando desconhecido ainda colapsa repeticao", () => {
  const r = filter("comando-estranho", "x\nx\nx");
  assert.equal(r.rule, null);
  assert.match(r.text, /×3/);
});
