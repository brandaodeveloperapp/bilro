import { test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { randomUUID } from "node:crypto";

const HOOK = join(import.meta.dirname, "..", "hooks", "preflight.js");

function run(payload) {
  return execFileSync("node", [HOOK], { input: JSON.stringify(payload), encoding: "utf8" });
}

const base = { tool_name: "Task", cwd: process.cwd(), tool_input: {} };

test("ignora ferramenta que nao e agente", () => {
  assert.equal(run({ ...base, tool_name: "Bash", session_id: randomUUID() }), "");
});

test("sempre informa o custo fixo", () => {
  const out = run({ ...base, session_id: randomUUID(), tool_input: { subagent_type: "x", prompt: "p" } });
  assert.match(out, /custo fixo/);
});

test("avisa quando o segundo agente repete o assunto", () => {
  const s = randomUUID();
  const input = { subagent_type: "ux-ds", description: "auditar listas react native", prompt: "medir FlatList ScrollView telas" };
  run({ ...base, session_id: s, tool_input: input });
  const out = run({ ...base, session_id: s, tool_input: { ...input, subagent_type: "code-reviewer" } });
  assert.match(out, /mesmo assunto/);
});

test("nao acusa repeticao em assunto diferente", () => {
  const s = randomUUID();
  run({ ...base, session_id: s, tool_input: { subagent_type: "a", description: "migrations postgres alembic", prompt: "criar tabela nova" } });
  const out = run({ ...base, session_id: s, tool_input: { subagent_type: "b", description: "deploy kubernetes rollout", prompt: "subir imagem nova" } });
  assert.doesNotMatch(out, /mesmo assunto/);
});

test("hook task encaminha para o preflight", () => {
  const out = execFileSync("node", [join(import.meta.dirname, "..", "bin", "bilro"), "hook", "task"], {
    input: JSON.stringify({ ...base, session_id: randomUUID(), tool_input: { subagent_type: "x", prompt: "p" } }),
    encoding: "utf8",
  });
  assert.match(out, /custo fixo/);
});

test("hook session resume a conta sem ler stdin", () => {
  const out = execFileSync("node", [join(import.meta.dirname, "..", "bin", "bilro"), "hook", "session"], {
    encoding: "utf8",
  });
  assert.match(out, /custo fixo por request/);
});
