use once_cell::sync::Lazy;
use regex::Regex;
use std::fs;
use std::io;
use std::path::Path;

static DECL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^\s*(export\s+)?(async\s+)?(function|class|const|let|var|interface|type|enum|def|struct|impl|trait|fn|public|private|protected|static|module|namespace|describe|it|test)\b|^\s*[\w$.]+\s*[:=]\s*(async\s*)?(\([^)]*\)|function)\s*(=>)?\s*\{?\s*$|^\s*(@|#\[)",
    )
    .unwrap()
});

static COMMENT_ABOVE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*(/\*\*|\*|//|#)").unwrap());

fn is_blank(line: &str) -> bool {
    line.chars().all(char::is_whitespace)
}

/// Whitespace-only compression. Meaning is untouched, so the result stays safe
/// to reason about byte for byte; the saving is small but costs nothing.
pub fn safe(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut blanks = 0u32;
    for line in text.split('\n') {
        let trimmed = line.trim_end();
        if is_blank(trimmed) {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push(trimmed.to_string());
    }
    out.join("\n")
}

/// Structure without bodies: declarations survive, the code between them is
/// replaced by a marker naming the exact line range, so anything elided can be
/// fetched precisely instead of guessed at.
pub fn outline(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut keep = vec![false; lines.len()];
    for i in 0..lines.len() {
        if DECL.is_match(lines[i]) {
            keep[i] = true;
            if i > 0 && COMMENT_ABOVE.is_match(lines[i - 1]) {
                keep[i - 1] = true;
            }
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut gap: Option<(usize, usize)> = None;
    let flush = |gap: &mut Option<(usize, usize)>, out: &mut Vec<String>| {
        let Some((start, end)) = *gap else { return };
        let n = end - start + 1;
        if n <= 2 {
            for line in &lines[start..=end] {
                out.push((*line).to_string());
            }
        } else {
            out.push(format!("      … {} lines ({}-{})", n, start + 1, end + 1));
        }
        *gap = None;
    };

    for (i, keep_line) in keep.iter().enumerate() {
        if *keep_line {
            flush(&mut gap, &mut out);
            out.push(lines[i].to_string());
        } else if let Some((_, end)) = gap.as_mut() {
            *end = i;
        } else {
            gap = Some((i, i));
        }
    }
    flush(&mut gap, &mut out);
    out.join("\n")
}

/// Outcome of reading a file: the transformed text plus enough metadata to
/// know how much was elided and whether it is still safe to edit from.
pub struct ReadResult {
    pub text: String,
    pub mode: String,
    pub lossy: bool,
    pub before: usize,
    pub after: usize,
    pub saved: f64,
}

pub fn read(path: &Path, mode: &str) -> io::Result<ReadResult> {
    let raw = fs::read_to_string(path)?;
    let text = if mode == "outline" { outline(&raw) } else { safe(&raw) };
    let before = raw.len();
    let after = text.len();
    let saved = if before > 0 { 1.0 - (after as f64 / before as f64) } else { 0.0 };
    Ok(ReadResult { text, mode: mode.to_string(), lossy: mode == "outline", before, after, saved })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_does_not_change_meaning_only_whitespace() {
        let src = "const a = 1;   \n\n\n\nconst b = 2;\n";
        let out = safe(src);
        assert!(out.contains("const a = 1;"));
        assert!(out.contains("const b = 2;"));
        assert!(!out.contains(" \n"));
        assert!(!out.contains("\n\n\n"));
    }

    #[test]
    fn outline_preserves_every_signature() {
        let src = [
            "export function alpha(x) {",
            "  const y = x + 1;",
            "  const z = y * 2;",
            "  return z;",
            "}",
            "export class Beta {",
            "  method() {",
            "    return 1;",
            "  }",
            "}",
        ]
        .join("\n");
        let out = outline(&src);
        assert!(out.contains("export function alpha"));
        assert!(out.contains("export class Beta"));
    }

    #[test]
    fn outline_states_the_exact_range_of_what_it_elided() {
        let mut lines = vec!["function f() {".to_string()];
        for i in 0..30 {
            lines.push(format!("  line {i}"));
        }
        lines.push("}".to_string());
        let src = lines.join("\n");
        let out = outline(&src);
        let marker = Regex::new(r"… \d+ lines \(\d+-\d+\)").unwrap();
        assert!(marker.is_match(&out));
        assert!(out.len() < src.len());
    }
}
