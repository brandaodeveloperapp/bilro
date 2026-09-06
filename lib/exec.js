import { execFileSync } from "node:child_process";

const SHELL_META = /[;&|`$><\n\r\\]|\$\(|\|\||&&/;

/** Splits a command line into argv, honouring quotes. No shell involved. */
export function toArgv(line) {
  const out = [];
  const re = /"([^"]*)"|'([^']*)'|(\S+)/g;
  let m;
  while ((m = re.exec(line))) out.push(m[1] ?? m[2] ?? m[3]);
  return out;
}

export function isSafe(line) {
  return !SHELL_META.test(line);
}

/**
 * Runs a declared command with no shell, so a value read from a data file
 * cannot chain, redirect or substitute its way into something else.
 */
export function runDeclared(line, { timeout = 30000, cwd } = {}) {
  if (!isSafe(line)) return { ok: false, refused: true, output: "usa sintaxe de shell" };
  const argv = toArgv(line);
  if (!argv.length) return { ok: false, refused: true, output: "vazio" };
  try {
    const output = execFileSync(argv[0], argv.slice(1), {
      encoding: "utf8",
      timeout,
      cwd,
      maxBuffer: 8 * 1024 * 1024,
    });
    return { ok: true, refused: false, output };
  } catch (e) {
    return { ok: false, refused: false, output: String(e.stdout || e.message || e).slice(0, 200) };
  }
}
