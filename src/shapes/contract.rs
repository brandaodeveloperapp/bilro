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
        return Applied { text: text.to_string(), shape: None, dropped: 0, confidence: 0.0 };
    };
    let r = (shape.compress)(&lines);
    if r.text.len() >= text.len() {
        return Applied { text: text.to_string(), shape: None, dropped: 0, confidence: 0.0 };
    }
    Applied { text: r.text, shape: Some(shape.name), dropped: r.dropped, confidence }
}
