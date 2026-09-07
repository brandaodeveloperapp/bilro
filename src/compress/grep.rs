use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;

static HIT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([^:]+):(\d+):(.*)$").unwrap());

struct OrderedMap<V> {
    order: Vec<String>,
    index: HashMap<String, usize>,
    values: Vec<V>,
}

impl<V> OrderedMap<V> {
    fn new() -> Self {
        Self { order: Vec::new(), index: HashMap::new(), values: Vec::new() }
    }

    fn entry_or_insert_with(&mut self, key: &str, make: impl FnOnce() -> V) -> &mut V {
        if let Some(&idx) = self.index.get(key) {
            return &mut self.values[idx];
        }
        let idx = self.values.len();
        self.index.insert(key.to_string(), idx);
        self.order.push(key.to_string());
        self.values.push(make());
        &mut self.values[idx]
    }

    fn iter(&self) -> impl Iterator<Item = (&String, &V)> {
        self.order.iter().map(move |k| (k, &self.values[self.index[k]]))
    }

    fn len(&self) -> usize {
        self.values.len()
    }
}

struct MatchEntry {
    count: usize,
    lines: Vec<String>,
}

/// Result of collapsing a raw grep output: what survived, and how much of the
/// repetition it stood for.
pub struct CompressResult {
    pub text: String,
    pub files: usize,
    pub hits: usize,
    pub dropped: i64,
}

const DEFAULT_PER_FILE: usize = 12;

pub fn compress(text: &str) -> CompressResult {
    compress_with(text, DEFAULT_PER_FILE)
}

/// Groups matches by file and collapses identical match text into one line
/// with a count. The matched content is the whole point of a search, so it is
/// never truncated away; only the repetition around it is.
pub fn compress_with(text: &str, per_file: usize) -> CompressResult {
    let lines: Vec<&str> = text.split('\n').filter(|l| !l.trim().is_empty()).collect();
    let mut files: OrderedMap<OrderedMap<MatchEntry>> = OrderedMap::new();
    let mut passthrough: Vec<&str> = Vec::new();

    for line in &lines {
        let Some(caps) = HIT.captures(line) else {
            passthrough.push(line);
            continue;
        };
        let file = &caps[1];
        let no = caps[2].to_string();
        let body = &caps[3];
        let key = body.trim();

        let seen = files.entry_or_insert_with(file, OrderedMap::new);
        let entry = seen.entry_or_insert_with(key, || MatchEntry { count: 0, lines: Vec::new() });
        entry.count += 1;
        if entry.lines.len() < 3 {
            entry.lines.push(no);
        }
    }

    if files.len() == 0 {
        return CompressResult { text: text.to_string(), files: 0, hits: 0, dropped: 0 };
    }

    let mut out: Vec<String> = Vec::new();
    let mut hits = 0usize;
    for (file, seen) in files.iter() {
        let entries: Vec<(&String, &MatchEntry)> = seen.iter().collect();
        let total: usize = entries.iter().map(|(_, e)| e.count).sum();
        hits += total;
        out.push(format!("{file}  {total}"));
        for (body, e) in entries.iter().take(per_file) {
            let extra =
                if e.count > e.lines.len() { format!("+{}", e.count - e.lines.len()) } else { String::new() };
            out.push(format!("  {}{}: {}", e.lines.join(","), extra, body));
        }
        if entries.len() > per_file {
            out.push(format!("  … {} other matches in this file", entries.len() - per_file));
        }
    }

    let compressed: String =
        passthrough.iter().map(|s| (*s).to_string()).chain(out).collect::<Vec<_>>().join("\n");
    let dropped = lines.len() as i64 - compressed.split('\n').count() as i64;
    CompressResult { text: compressed, files: files.len(), hits, dropped }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_content_is_never_truncated() {
        let raw = "a.ts:1:const token = process.env.SECRET\nb.ts:9:const token = process.env.SECRET";
        let r = compress(raw);
        assert!(r.text.contains("const token = process.env.SECRET"));
        assert_eq!(r.hits, 2);
    }

    #[test]
    fn identical_match_across_many_lines_becomes_one_line_with_a_count() {
        let raw = (0..20).map(|i| format!("a.ts:{}:import x", i + 1)).collect::<Vec<_>>().join("\n");
        let r = compress(&raw);
        assert_eq!(r.hits, 20);
        assert!(r.text.len() < raw.len());
        assert!(r.text.contains("+17"));
    }

    #[test]
    fn file_with_many_distinct_matches_says_how_many_were_left_out() {
        let raw =
            (0..40).map(|i| format!("a.ts:{}:unique line {}", i + 1, i)).collect::<Vec<_>>().join("\n");
        let r = compress_with(&raw, 5);
        assert!(r.text.contains("… 35 other matches"));
    }

    #[test]
    fn output_that_is_not_grep_passes_through_untouched() {
        let raw = "this has no match format\nneither does this";
        assert_eq!(compress(raw).text, raw);
    }
}
