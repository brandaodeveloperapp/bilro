//! Running a child process without letting it hang or fill a pipe. Nothing
//! here decides whether a command *may* run — that judgement belongs to the
//! layer above, and keeping it out of here is what stops the store depending
//! on the runner.

use std::io::Read;
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

/// Spawns `command`, always with piped stdio and no shell, and enforces
/// `timeout_ms` by polling and killing on deadline instead of blocking
/// forever. `Command` has no native timeout, so this is that timeout.
pub fn spawn_with_timeout(
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

