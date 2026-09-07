use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub struct Runtime {
    pub language: &'static str,
    pub program: &'static str,
    pub extension: &'static str,
    pub args_before_file: &'static [&'static str],
}

const RUNTIMES: &[Runtime] = &[
    Runtime { language: "shell", program: "/bin/sh", extension: "sh", args_before_file: &[] },
    Runtime { language: "javascript", program: "node", extension: "mjs", args_before_file: &[] },
    Runtime { language: "python", program: "python3", extension: "py", args_before_file: &[] },
    Runtime { language: "ruby", program: "ruby", extension: "rb", args_before_file: &[] },
    Runtime { language: "perl", program: "perl", extension: "pl", args_before_file: &[] },
];

pub fn runtime_for(language: &str) -> Option<&'static Runtime> {
    let wanted = match language {
        "js" | "node" => "javascript",
        "py" | "python3" => "python",
        "sh" | "bash" => "shell",
        other => other,
    };
    RUNTIMES.iter().find(|r| r.language == wanted)
}

pub fn languages() -> Vec<&'static str> {
    RUNTIMES.iter().map(|r| r.language).collect()
}

/// Whether the interpreter for a language is actually installed. Reporting that
/// up front beats a confusing "no such file" from deep inside a run.
pub fn available(runtime: &Runtime) -> bool {
    if runtime.program.starts_with('/') {
        return Path::new(runtime.program).exists();
    }
    Command::new("which")
        .arg(runtime.program)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Debug)]
pub struct ScriptResult {
    pub output: String,
    pub failed: bool,
    pub timed_out: bool,
}

/// Owns the scratch file for as long as the run needs it and removes it on the
/// way out, including the paths that return early with an error.
pub struct Scratch(PathBuf);

impl Scratch {
    pub fn new(extension: &str, code: &str) -> std::io::Result<Scratch> {
        let path = scratch(extension);
        let mut f = std::fs::File::create(&path)?;
        f.write_all(code.as_bytes())?;
        Ok(Scratch(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn scratch(extension: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "bilro-script-{}-{}.{extension}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ))
}

/// Runs a snippet in its own interpreter and returns what it printed. The code
/// is written to a scratch file rather than passed as an argument, so quoting
/// never has to survive a shell that is not involved in the first place.
pub fn run(language: &str, code: &str, cwd: Option<&Path>, timeout_ms: Option<u64>) -> Result<ScriptResult, String> {
    let runtime = runtime_for(language)
        .ok_or_else(|| format!("unsupported language: {language}. Available: {}", languages().join(", ")))?;
    if !available(runtime) {
        return Err(format!("{} is not installed on this machine", runtime.program));
    }

    let scratch = Scratch::new(runtime.extension, code)
        .map_err(|e| format!("could not write the script: {e}"))?;
    let file = scratch.path().to_path_buf();

    let mut cmd = Command::new(runtime.program);
    cmd.args(runtime.args_before_file).arg(&file).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Err(format!("could not run {}: {e}", runtime.program)),
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let drain = |pipe: Option<std::process::ChildStdout>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                use std::io::Read;
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let drain_err = |pipe: Option<std::process::ChildStderr>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                use std::io::Read;
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out_handle = drain(stdout);
    let err_handle = drain_err(stderr);

    let deadline = std::time::Instant::now()
        + std::time::Duration::from_millis(timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    timed_out = true;
                    break child.wait().map_err(|e| e.to_string())?;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) => return Err(format!("failed waiting for the process: {e}")),
        }
    };

    let stdout_bytes = out_handle.join().unwrap_or_default();
    let stderr_bytes = err_handle.join().unwrap_or_default();

    let mut text = String::from_utf8_lossy(&stdout_bytes).to_string();
    let err = String::from_utf8_lossy(&stderr_bytes);
    if !err.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&err);
    }
    if timed_out {
        text.push_str(&format!("\n[bilro: interrupted after {}ms]", timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS)));
    }
    Ok(ScriptResult { output: crate::redact::redact(&text), failed: !status.success() || timed_out, timed_out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_alias_resolves() {
        assert_eq!(runtime_for("js").unwrap().language, "javascript");
        assert_eq!(runtime_for("py").unwrap().language, "python");
        assert_eq!(runtime_for("bash").unwrap().language, "shell");
        assert!(runtime_for("cobol").is_none());
    }

    #[test]
    fn shell_always_runs() {
        let r = run("shell", "echo bilro", None, None).unwrap();
        assert!(r.output.contains("bilro"));
        assert!(!r.failed);
    }

    #[test]
    fn javascript_runs_when_node_exists() {
        let rt = runtime_for("javascript").unwrap();
        if !available(rt) {
            return;
        }
        let r = run("javascript", "console.log(6*7)", None, None).unwrap();
        assert!(r.output.contains("42"), "got: {}", r.output);
    }

    #[test]
    fn python_runs_when_installed() {
        let rt = runtime_for("python").unwrap();
        if !available(rt) {
            return;
        }
        let r = run("python", "print(6*7)", None, None).unwrap();
        assert!(r.output.contains("42"), "got: {}", r.output);
    }

    #[test]
    fn script_error_comes_back_with_the_message_and_marked_as_failed() {
        let r = run("shell", "echo before; this_command_does_not_exist", None, None).unwrap();
        assert!(r.output.contains("before"), "stdout has to survive");
        assert!(r.failed);
    }

    #[test]
    fn infinite_loop_is_interrupted_instead_of_hanging() {
        let start = std::time::Instant::now();
        let r = run("shell", "while true; do :; done", None, Some(300)).unwrap();
        assert!(r.timed_out);
        assert!(start.elapsed().as_secs() < 10, "took too long to interrupt");
    }

    #[test]
    fn code_with_quotes_does_not_need_to_survive_a_shell() {
        let r = run("shell", "printf '%s\\n' \"double \\\"quotes\\\" and 'single'\"", None, None).unwrap();
        assert!(r.output.contains("quotes"), "got: {}", r.output);
    }

    #[test]
    fn unknown_language_lists_the_available_ones() {
        let e = run("cobol", "DISPLAY 1", None, None).unwrap_err();
        assert!(e.contains("shell") && e.contains("python"), "got: {e}");
    }

    #[test]
    fn secret_in_what_the_script_prints_is_masked() {
        let r = run("shell", "echo 'DB=postgres://u:S3CR3T@h:5432/d'", None, None).unwrap();
        assert!(!r.output.contains("S3CR3T"), "leaked: {}", r.output);
    }

    #[test]
    fn temp_file_is_removed_even_when_the_run_fails() {
        let path;
        {
            let s = Scratch::new("sh", "echo x").unwrap();
            path = s.path().to_path_buf();
            assert!(path.exists(), "should exist while in use");
        }
        assert!(!path.exists(), "left behind {}", path.display());
    }

    #[test]
    fn each_run_uses_its_own_file() {
        let a = Scratch::new("sh", "echo a").unwrap();
        let b = Scratch::new("sh", "echo b").unwrap();
        assert_ne!(a.path(), b.path());
    }
}

#[cfg(test)]
mod pipe_tests {
    use super::*;

    #[test]
    fn output_larger_than_the_pipe_buffer_does_not_hang() {
        let start = std::time::Instant::now();
        let r = run("shell", "seq 1 200000", None, Some(30_000)).unwrap();
        assert!(!r.timed_out, "hung waiting for the pipe to drain");
        assert!(r.output.lines().count() > 199_000, "lost lines: {}", r.output.lines().count());
        assert!(start.elapsed().as_secs() < 15, "took {}s", start.elapsed().as_secs());
    }
}
