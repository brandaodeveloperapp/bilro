use crate::compress::shapes::contract::Shape;
use std::path::PathBuf;
use std::process::Command;

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}


pub fn learn_db() -> PathBuf {
    home().join(".claude").join("bilro").join("learn.db")
}


pub fn all_shapes() -> Vec<Shape> {
    vec![
        crate::compress::shapes::table::shape(),
        crate::compress::shapes::diff::shape(),
        crate::compress::shapes::listing::shape(),
        crate::compress::shapes::install_log::shape(),
        crate::compress::shapes::keyvalue::shape(),
        crate::compress::shapes::test_report::shape(),
        crate::compress::shapes::diagnostics::shape(),
    ]
}


/// Structure first, then history: a shape works on a command never seen before,
/// while denoise needs several runs before it may judge anything. Both report
/// what they removed — a shape that collapses forty array items in silence
/// breaks the same promise a filter that eats an error line breaks.
pub fn squeeze(command: &str, output: &str) -> (String, String) {
    let shaped = crate::compress::shapes::apply(&all_shapes(), output);
    let mut note = match (shaped.shape, shaped.note.trim()) {
        (Some(name), "") => name.to_string(),
        (Some(_), detail) => detail.to_string(),
        (None, _) => String::new(),
    };
    let Ok(db) = crate::compress::learn::open(&learn_db()) else {
        return (shaped.text, note);
    };
    match crate::compress::learn::denoise(&db, command, &shaped.text, 3, 0.8) {
        Ok(d) if d.learned && d.dropped > 0 => {
            if !note.is_empty() {
                note.push_str(" + ");
            }
            note.push_str(&format!("{} repeated lines", d.dropped));
            (d.text, note)
        }
        _ => (shaped.text, note),
    }
}


/// Says what happened when compression leaves nothing to print. Printing an
/// empty result would look like the command produced no output at all, which is
/// the silent loss this tool exists to avoid.
pub fn suppressed_notice(raw: &str, text: &str) -> Option<String> {
    if !text.trim().is_empty() || raw.trim().is_empty() {
        return None;
    }
    let lines = raw.lines().filter(|l| !l.trim().is_empty()).count();
    Some(format!(
        "identical to what this command already printed before: {lines} lines suppressed. bilro saw no failure among them, but it only recognises the ones it can name — run without bilro if the result matters"
    ))
}


/// Runs a command and returns its compressed output, the shape that did the
/// compressing, how much smaller it got, and the exit code. The code is not
/// decoration: a caller with no other channel — the MCP tool — would otherwise
/// read a failed command's silence as success. A command that runs out of time
/// still hands back everything it printed first: throwing that away to report
/// only the timeout is the silent loss this whole tool exists to prevent.
/// The deadline the MCP tool asks for. The command line does NOT get one: the
/// Bash hook rewrites `npm ci`, `cargo build` and `pytest` into `bilro filter`,
/// and a build that legitimately runs for four minutes must not be killed at
/// two — least of all with SIGKILL on the whole process group, which leaves a
/// half-written `node_modules` behind and reports exit 124 as the reason.
pub const MCP_TIMEOUT_MS: u64 = 120_000;

pub fn filtered(command: &str) -> Result<(String, String, usize, i32), String> {
    filtered_within(command, None)
}

pub fn filtered_within(command: &str, timeout_ms: Option<u64>) -> Result<(String, String, usize, i32), String> {
    let mut sh = Command::new("sh");
    sh.arg("-c").arg(command);
    let deadline = timeout_ms.unwrap_or(u64::MAX);
    let out = crate::proc::spawn_with_timeout(sh, deadline).map_err(|e| e.to_string())?;
    let code = if out.timed_out { 124 } else { out.status.and_then(|s| s.code()).unwrap_or(1) };
    let mut captured = String::from_utf8_lossy(&out.stdout).to_string();
    if !captured.is_empty() && !captured.ends_with('\n') {
        captured.push('\n');
    }
    captured.push_str(&String::from_utf8_lossy(&out.stderr));
    let raw = crate::redact::redact(&captured);
    let before = raw.len();
    let (text, note) = squeeze(command, &raw);
    if let Ok(mut db) = crate::compress::learn::open(&learn_db()) {
        let _ = crate::compress::learn::observe(&mut db, command, &raw);
    }
    let saved = if before > text.len() { 100 - text.len() * 100 / before.max(1) } else { 0 };
    let mut text = match suppressed_notice(&raw, &text) {
        Some(msg) => msg,
        None => text,
    };
    if out.timed_out {
        text.push_str(&format!(
            "\n[bilro: no answer after {}s, everything printed up to that point is above]",
            deadline / 1000
        ));
    }
    Ok((text, note, saved, code))
}


#[cfg(test)]
mod exit_code_tests {
    use super::*;

    #[test]
    fn a_failing_command_reports_its_code_even_when_it_printed_nothing() {
        let (_, _, _, code) = filtered("exit 3").unwrap();
        assert_eq!(code, 3);
        let (_, _, _, code) = filtered("false").unwrap();
        assert_eq!(code, 1);
    }

    /// `signature()` folds every integer to `N`, so a numeric suffix collides
    /// across runs and the third one gets denoised away. Letters do not fold.
    fn unique() -> String {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        n.to_string().bytes().map(|b| (b'a' + (b - b'0')) as char).collect()
    }

    #[test]
    fn a_successful_command_reports_zero() {
        let (text, _, _, code) = filtered(&format!("echo done-{}", unique())).unwrap();
        assert_eq!(code, 0);
        assert!(text.contains("done-"), "{text}");
    }

    #[test]
    fn stdout_without_a_final_newline_does_not_swallow_the_first_stderr_line() {
        let (text, _, _, _) =
            filtered(&format!("printf 'Building{}'; printf 'ERROR: linker failed\\n' >&2", unique()))
                .unwrap();
        assert!(text.contains("Building"), "{text}");
        assert!(
            crate::compress::learn::is_severe_text(&text),
            "the stderr line fused into stdout and stopped reading as a failure: {text}"
        );
    }
}
