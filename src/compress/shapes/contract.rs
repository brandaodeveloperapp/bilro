/// A shape is a way output is structured, not a tool that produced it. Dozens
/// of programs emit the same columnar table or the same file:line diagnostic
/// list, so compressing by shape covers tools nobody wrote a rule for.
pub const MIN_CONFIDENCE: f64 = 0.6;

pub struct Compressed {
    pub text: String,
    pub dropped: usize,
    pub note: String,
}

impl Compressed {
    pub fn unchanged(lines: &[&str]) -> Self {
        Compressed { text: lines.join("\n"), dropped: 0, note: String::new() }
    }
}

/// Every shape answers how confident it is that output has its structure, and
/// how to shrink it. Two rules are inviolable: a line `learn::is_severe` calls a
/// failure is never dropped, and surviving lines keep their order.
pub struct Shape {
    pub name: &'static str,
    pub detect: fn(&[&str]) -> f64,
    pub compress: fn(&[&str]) -> Compressed,
}

pub struct Applied {
    pub text: String,
    pub shape: Option<&'static str>,
    pub dropped: usize,
    pub confidence: f64,
    pub note: String,
}

/// The rule every shape declares and one of them broke: a line reporting a
/// failure is never dropped. Enforcing it once here rather than trusting seven
/// compressors means the array collapse that deleted three aborted CI jobs
/// cannot happen again in the eighth. What is re-attached is the input line
/// whose own failure evidence is nowhere in the output — checking for the whole
/// line instead would re-attach a JSON document that was faithfully reshaped.
fn keep_the_failures(lines: &[&str], result: Compressed) -> Compressed {
    let missing: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| {
            let trimmed = l.trim();
            if trimmed.is_empty() || result.text.contains(trimmed) {
                return false;
            }
            crate::compress::learn::severe_evidence(l)
                .is_some_and(|ev| !result.text.contains(ev.as_str()))
        })
        .collect();
    if missing.is_empty() {
        return result;
    }
    let mut text = result.text;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for l in &missing {
        text.push_str(l.trim());
        text.push('\n');
    }
    let note = format!("{}; {} failure line(s) kept back", result.note, missing.len());
    Compressed { text, dropped: result.dropped.saturating_sub(missing.len()), note }
}

pub fn apply(shapes: &[Shape], text: &str) -> Applied {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut best: Option<(&Shape, f64)> = None;
    for s in shapes {
        let c = (s.detect)(&lines);
        if c >= MIN_CONFIDENCE && best.is_none_or(|(_, b)| c > b) {
            best = Some((s, c));
        }
    }
    let Some((shape, confidence)) = best else {
        return Applied { text: text.to_string(), shape: None, dropped: 0, confidence: 0.0, note: String::new() };
    };
    let r = keep_the_failures(&lines, (shape.compress)(&lines));
    if r.text.len() >= text.len() {
        return Applied { text: text.to_string(), shape: None, dropped: 0, confidence: 0.0, note: String::new() };
    }
    Applied { text: r.text, shape: Some(shape.name), dropped: r.dropped, confidence, note: r.note }
}
