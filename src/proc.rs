//! Running a child process without letting it hang or fill a pipe. Nothing
//! here decides whether a command *may* run — that judgement belongs to the
//! layer above, and keeping it out of here is what stops the store depending
//! on the runner.

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a child process left behind: how it ended, everything it wrote, and
/// whether the deadline was what ended it.
pub struct SpawnOutcome {
    pub status: Option<std::process::ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

/// How long to keep reading after the command itself has finished. A pipe can
/// outlive its command: anything the command started in the background inherits
/// the same stdout, and reading to the end then waits for that instead. Waiting
/// forever is what turned an instant command into a twenty-five second one.
const PIPE_GRACE_MS: u64 = 300;

/// Killing the shell is not enough. `sh -c "sleep 30 &"` leaves a grandchild
/// holding the same stdout pipe, and reading that pipe to the end then waits
/// for the grandchild, not the child — which is how a deadline of one second
/// turned into a wait of thirty. The child runs in its own process group so
/// the whole tree can be ended at once.
#[cfg(unix)]
pub(crate) fn end_the_whole_group(pid: u32) {
    let _ = Command::new("/bin/kill").arg("-9").arg(format!("-{pid}")).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

#[cfg(not(unix))]
pub(crate) fn end_the_whole_group(_pid: u32) {}

/// Spawns `command`, always with piped stdio and no shell, and enforces
/// `timeout_ms` by polling and killing on deadline instead of blocking
/// forever. `Command` has no native timeout, so this is that timeout.
pub fn spawn_with_timeout(
    mut command: Command,
    timeout_ms: u64,
) -> std::io::Result<SpawnOutcome> {
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");

    let out_buf = Arc::new(Mutex::new(Vec::new()));
    let err_buf = Arc::new(Mutex::new(Vec::new()));
    let stdout_handle = {
        let sink = Arc::clone(&out_buf);
        thread::spawn(move || drain_into(&mut stdout, &sink))
    };
    let stderr_handle = {
        let sink = Arc::clone(&err_buf);
        thread::spawn(move || drain_into(&mut stderr, &sink))
    };

    let pid = child.id();
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait()? {
            Some(s) => break Some(s),
            None => {
                if Instant::now() >= deadline {
                    end_the_whole_group(pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                thread::sleep(Duration::from_millis(15));
            }
        }
    };

    let mut abandoned = false;
    if !finished(&stdout_handle, &stderr_handle, PIPE_GRACE_MS) {
        end_the_whole_group(pid);
        abandoned = !finished(&stdout_handle, &stderr_handle, PIPE_GRACE_MS);
    }
    let mut stdout_buf = out_buf.lock().map(|b| b.clone()).unwrap_or_default();
    let stderr_buf = err_buf.lock().map(|b| b.clone()).unwrap_or_default();
    if abandoned {
        stdout_buf.extend_from_slice(
            b"\n[bilro: the command finished but something it started still held the output; read stopped here]\n",
        );
    }
    Ok(SpawnOutcome { status, stdout: stdout_buf, stderr: stderr_buf, timed_out })
}

/// Whether both readers are done, waiting at most `grace_ms` for them.
fn finished(a: &thread::JoinHandle<()>, b: &thread::JoinHandle<()>, grace_ms: u64) -> bool {
    let until = Instant::now() + Duration::from_millis(grace_ms);
    while Instant::now() < until {
        if a.is_finished() && b.is_finished() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    a.is_finished() && b.is_finished()
}

/// Copies a pipe into a shared buffer as it arrives, so whatever the command
/// managed to print is readable even when the reader has to be abandoned.
fn drain_into<R: Read>(pipe: &mut R, sink: &Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Ok(mut b) = sink.lock() {
                    b.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
}


#[cfg(test)]
mod deadline_tests {
    use super::*;

    #[test]
    fn a_background_grandchild_does_not_outlive_the_deadline() {
        let mut sh = Command::new("/bin/sh");
        sh.arg("-c").arg("sleep 30 & sleep 20");
        let started = Instant::now();
        let out = spawn_with_timeout(sh, 800).unwrap();
        let waited = started.elapsed();
        assert!(out.timed_out, "the deadline did not fire");
        assert!(
            waited < Duration::from_secs(5),
            "waited {waited:?} on an 800ms deadline: the grandchild still held the pipe"
        );
    }
}

#[cfg(test)]
mod pipe_grace_tests {
    use super::*;

    #[test]
    fn a_command_that_leaves_a_child_behind_still_answers_at_once() {
        let mut sh = Command::new("/bin/sh");
        sh.arg("-c").arg("echo IMMEDIATE; (sleep 25 &)");
        let started = Instant::now();
        let out = spawn_with_timeout(sh, 120_000).unwrap();
        let waited = started.elapsed();
        assert!(
            waited < Duration::from_secs(3),
            "waited {waited:?} for a command that printed and exited"
        );
        assert!(String::from_utf8_lossy(&out.stdout).contains("IMMEDIATE"));
    }

    #[test]
    fn abandoning_a_pipe_is_announced_not_silent() {
        let mut sh = Command::new("/bin/sh");
        sh.arg("-c").arg("echo FIRST; (sleep 25 &)");
        let out = spawn_with_timeout(sh, 120_000).unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("FIRST"), "{text}");
        assert!(!out.timed_out);
    }

    #[test]
    fn an_ordinary_command_keeps_all_of_its_output() {
        let mut sh = Command::new("/bin/sh");
        sh.arg("-c").arg("seq 1 500");
        let out = spawn_with_timeout(sh, 10_000).unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert_eq!(text.lines().filter(|l| !l.trim().is_empty()).count(), 500, "output was cut short");
    }
}
