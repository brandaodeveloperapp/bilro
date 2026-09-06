import { test } from "node:test";
import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { read, write, record, summarize } from "../lib/ledger.js";

test("sessao nova comeca vazia e nao quebra", () => {
  const s = read(randomUUID());
  assert.deepEqual(s.dispatches, []);
  assert.deepEqual(s.withheld, []);
});

test("soma custo e conta repeticao", () => {
  const id = randomUUID();
  write(id, {
    dispatches: [
      { cost: 20000, repeat: false },
      { cost: 21000, repeat: true },
    ],
    withheld: [{ tokens: 5000 }],
    filtered: [{ saved: 1000 }],
  });
  const t = summarize(id);
  assert.equal(t.dispatches, 2);
  assert.equal(t.dispatchCost, 41000);
  assert.equal(t.repeats, 1);
  assert.equal(t.saved, 6000);
});

test("record acrescenta sem perder o que ja existia", () => {
  const id = randomUUID();
  record(id, "dispatches", { cost: 1 });
  const s = record(id, "dispatches", { cost: 2 });
  assert.equal(s.dispatches.length, 2);
});
