import { execFileSync } from "node:child_process";

const ALLOWED = new Set([
  "grep", "egrep", "fgrep", "rg", "cat", "head", "tail", "ls", "wc", "stat", "file", "jq",
  "plutil", "uname", "sw_vers", "date", "echo", "test", "true", "false", "dirname", "basename",
  "realpath", "readlink", "printenv", "which", "sort", "uniq", "cut", "tr", "diff", "cmp",
  "md5", "shasum", "git",
]);

const GIT_SUBCOMMANDS = new Set([
  "rev-parse", "log", "status", "describe", "show", "diff", "branch", "tag", "remote",
  "ls-files", "ls-remote", "cat-file", "rev-list", "symbolic-ref", "shortlog", "blame",
]);

const GIT_FORBIDDEN_FLAGS = [
  "-c", "--config", "--exec-path", "-C", "--upload-pack", "--receive-pack", "--namespace",
  "--git-dir", "--work-tree",
];

/**
 * git is allowed because a version check is the commonest thing a memory wants
 * to assert, but git is also an execution engine: an alias beginning with `!`
 * runs through a shell, and `-c` can define one inline.
 */
function gitIsReadOnly(args) {
  if (args.some((a) => GIT_FORBIDDEN_FLAGS.some((f) => a === f || a.startsWith(`${f}=`)))) return false;
  const sub = args.find((a) => !a.startsWith("-"));
  return Boolean(sub) && GIT_SUBCOMMANDS.has(sub);
}

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

/** The program part of a declared command, without its directory. */
export function programOf(line) {
  const argv = toArgv(line);
  if (!argv.length) return null;
  return argv[0].split("/").pop();
}

/**
 * Refusing shell syntax is not enough on its own: `sh -c "..."` carries no
 * metacharacter and still hands the whole line to a shell, so the program
 * itself has to be vouched for.
 */
export function isAllowedProgram(line) {
  const p = programOf(line);
  if (!p || !ALLOWED.has(p)) return false;
  if (p === "git") return gitIsReadOnly(toArgv(line).slice(1));
  return true;
}

/**
 * Runs a declared command with no shell, so a value read from a data file
 * cannot chain, redirect or substitute its way into something else.
 */
export function runDeclared(line, { timeout = 30000, cwd } = {}) {
  if (!isSafe(line)) return { ok: false, refused: true, output: "usa sintaxe de shell" };
  if (!isAllowedProgram(line))
    return { ok: false, refused: true, output: `programa nao permitido em verify: ${programOf(line)}` };
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
