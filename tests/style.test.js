import { test } from "node:test";
import assert from "node:assert/strict";
import { ruleset, LEVELS } from "../lib/style.js";

test("off nao injeta nada", () => {
  assert.equal(ruleset("off"), "");
});

test("terse carrega tudo que lean carrega", () => {
  const lean = ruleset("lean");
  const terse = ruleset("terse");
  const primeira = lean.split("\n")[1];
  assert.ok(terse.includes(primeira));
  assert.ok(terse.length > lean.length);
});

test("todo nivel ligado preserva as excecoes", () => {
  for (const l of LEVELS.filter((l) => l !== "off")) {
    assert.match(ruleset(l), /aviso de seguranca|seguran/i);
    assert.match(ruleset(l), /commit/);
  }
});

test("todo nivel ligado carrega a regra anti-deriva", () => {
  for (const l of LEVELS.filter((l) => l !== "off")) {
    assert.match(ruleset(l), /TODA resposta/);
  }
});
