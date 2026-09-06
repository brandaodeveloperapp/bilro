import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const HOOK = join(dirname(fileURLToPath(import.meta.url)), "..", "hooks", "shadow.js");
const fire = (payload) =>
  execFileSync(process.execPath, [HOOK], { input: JSON.stringify(payload), encoding: "utf8" });

test("nao imprime nada, para nao poluir o contexto que promete economizar", () => {
  const out = fire({ tool_name: "Bash", tool_input: { command: "echo oi" }, tool_response: { stdout: "oi\n" } });
  assert.equal(out, "");
});

test("json quebrado nao derruba a chamada do usuario", () => {
  const out = execFileSync(process.execPath, [HOOK], { input: "{nao e json", encoding: "utf8" });
  assert.equal(out, "");
});

test("ignora ferramenta que nao e Bash", () => {
  assert.equal(fire({ tool_name: "Read", tool_input: { command: "x" }, tool_response: { stdout: "y" } }), "");
});

test("sobrevive a payload sem output", () => {
  assert.equal(fire({ tool_name: "Bash", tool_input: { command: "true" }, tool_response: {} }), "");
});
