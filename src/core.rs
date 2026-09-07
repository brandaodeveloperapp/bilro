use crate::shapes::contract::Shape;
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
        crate::shapes::table::shape(),
        crate::shapes::diff::shape(),
        crate::shapes::listing::shape(),
        crate::shapes::install_log::shape(),
        crate::shapes::keyvalue::shape(),
        crate::shapes::test_report::shape(),
        crate::shapes::diagnostics::shape(),
    ]
}


/// Structure first, then history: a shape works on a command never seen before,
/// while denoise needs several runs before it may judge anything.
pub fn squeeze(command: &str, output: &str) -> (String, String) {
    let shaped = crate::shapes::apply(&all_shapes(), output);
    let mut note = shaped.shape.map(|s| s.to_string()).unwrap_or_default();
    let Ok(db) = crate::learn::open(&learn_db()) else {
        return (shaped.text, note);
    };
    match crate::learn::denoise(&db, command, &shaped.text, 3, 0.8) {
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
/// compressing, and how much smaller it got. Shared by the command line and the
/// MCP tool so both answer identically.
pub fn filtered(command: &str) -> Result<(String, String, usize), String> {
    let out = Command::new("sh").arg("-c").arg(command).output().map_err(|e| e.to_string())?;
    let mut captured = String::from_utf8_lossy(&out.stdout).to_string();
    captured.push_str(&String::from_utf8_lossy(&out.stderr));
    let raw = crate::redact::redact(&captured);
    let before = raw.len();
    let (text, note) = squeeze(command, &raw);
    if let Ok(mut db) = crate::learn::open(&learn_db()) {
        let _ = crate::learn::observe(&mut db, command, &raw);
    }
    let saved = if before > text.len() { 100 - text.len() * 100 / before.max(1) } else { 0 };
    let text = match suppressed_notice(&raw, &text) {
        Some(msg) => msg,
        None => text,
    };
    Ok((text, note, saved))
}

