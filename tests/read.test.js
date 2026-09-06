import test from "node:test";
import assert from "node:assert/strict";
import { safe, outline } from "../lib/read.js";

test("safe nao muda significado, so espaco", () => {
  const src = "const a = 1;   \n\n\n\nconst b = 2;\n";
  const out = safe(src);
  assert.ok(out.includes("const a = 1;"));
  assert.ok(out.includes("const b = 2;"));
  assert.ok(!/ \n/.test(out));
  assert.ok(!/\n\n\n/.test(out));
});

test("outline preserva toda assinatura", () => {
  const src = [
    "export function alpha(x) {",
    "  const y = x + 1;",
    "  const z = y * 2;",
    "  return z;",
    "}",
    "export class Beta {",
    "  metodo() {",
    "    return 1;",
    "  }",
    "}",
  ].join("\n");
  const out = outline(src);
  assert.match(out, /export function alpha/);
  assert.match(out, /export class Beta/);
});

test("outline diz a faixa exata do que elidiu, para poder buscar depois", () => {
  const src = ["function f() {", ...Array.from({ length: 30 }, (_, i) => `  linha ${i}`), "}"].join("\n");
  const out = outline(src);
  assert.match(out, /… \d+ linhas \(\d+-\d+\)/);
  assert.ok(out.length < src.length);
});
