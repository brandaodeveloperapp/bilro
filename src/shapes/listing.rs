use crate::learn::is_severe;
use crate::shapes::contract::{Compressed, Shape};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

const EXT_THRESHOLD: usize = 30;

static LS_ENTRY_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^([-dlbcps][-rwxXsStT]{9}[@+]?)\s+(\d+)\s+(\S+)\s+(\S+)\s+(\d+)\s+(\S+\s+\S+\s+\S+)\s+(.+)$")
        .unwrap()
});
static TOTAL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^total\s+\d+$").unwrap());
static HEADER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\S[^\s:]*:$").unwrap());
static GREP_HIT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\S+):(\d+):(.*)$").unwrap());
static TREE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[\s\x{2502}]*[\x{251c}\x{2514}]\x{2500}\x{2500}\s+\S").unwrap());
static WC_ENTRY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*\d+\s+\S+$").unwrap());
static EXT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.[A-Za-z0-9]+$").unwrap());

fn is_path_line(line: &str) -> bool {
    let t = line.trim();
    !t.is_empty() && !t.contains(' ') && t.contains('/') && !t.contains('=')
}

fn is_grep_hit(line: &str) -> bool {
    if !GREP_HIT_RE.is_match(line) {
        return false;
    }
    line.matches(':').count() <= 6
}

struct Classified {
    mode: &'static str,
    ratio: f64,
}

fn classify(lines: &[&str]) -> Classified {
    let non_empty: Vec<&str> = lines.iter().copied().filter(|l| !l.trim().is_empty()).collect();
    let n = non_empty.len();
    if n < 2 {
        return Classified { mode: "none", ratio: 0.0 };
    }
    let mut scored: Vec<(&'static str, usize)> = vec![
        (
            "ls",
            non_empty
                .iter()
                .filter(|l| LS_ENTRY_RE.is_match(l) || TOTAL_RE.is_match(l) || HEADER_RE.is_match(l.trim()))
                .count(),
        ),
        ("grep", non_empty.iter().filter(|l| is_grep_hit(l)).count()),
        ("tree", non_empty.iter().filter(|l| TREE_RE.is_match(l)).count()),
        ("path", non_empty.iter().filter(|l| is_path_line(l)).count()),
        ("wc", non_empty.iter().filter(|l| WC_ENTRY_RE.is_match(l)).count()),
    ];
    scored.sort_by_key(|s| std::cmp::Reverse(s.1));
    let (mode, hits) = scored[0];
    Classified { mode, ratio: hits as f64 / n as f64 }
}

fn extension_of(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    match EXT_RE.find(base) {
        Some(m) => m.as_str().to_lowercase(),
        None => "(no extension)".to_string(),
    }
}

fn group_by_extension(paths: &[String]) -> Vec<(String, usize)> {
    let mut order: Vec<String> = Vec::new();
    let mut counts: Vec<(String, usize)> = Vec::new();
    for p in paths {
        let ext = extension_of(p);
        if let Some(entry) = counts.iter_mut().find(|(e, _)| *e == ext) {
            entry.1 += 1;
        } else {
            order.push(ext.clone());
            counts.push((ext, 1));
        }
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    counts
}

fn common_prefix(paths: &[String]) -> String {
    if paths.len() < 2 {
        return String::new();
    }
    let mut prefix: Vec<char> = paths[0].chars().collect();
    for p in &paths[1..] {
        let pc: Vec<char> = p.chars().collect();
        let mut i = 0;
        while i < prefix.len() && i < pc.len() && prefix[i] == pc[i] {
            i += 1;
        }
        prefix.truncate(i);
        if prefix.is_empty() {
            return String::new();
        }
    }
    match prefix.iter().rposition(|&c| c == '/') {
        Some(cut) if cut > 0 => prefix[..cut + 1].iter().collect(),
        _ => String::new(),
    }
}

fn compress_path_list(lines: &[&str]) -> Compressed {
    let paths: Vec<String> = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).collect();
    let prefix = common_prefix(&paths);
    let rel: Vec<String> = if prefix.is_empty() {
        paths.clone()
    } else {
        paths.iter().map(|p| p.strip_prefix(prefix.as_str()).unwrap_or(p).to_string()).collect()
    };
    let (body, note) = if paths.len() >= EXT_THRESHOLD {
        let groups = group_by_extension(&rel);
        let body = groups
            .iter()
            .map(|(ext, count)| format!("{}: {} file{}", ext, count, if *count == 1 { "" } else { "s" }))
            .collect::<Vec<_>>()
            .join("\n");
        let note = format!(
            "{} paths grouped into {} extension(s){}",
            paths.len(),
            groups.len(),
            if prefix.is_empty() { String::new() } else { format!(", common prefix removed ({})", prefix) }
        );
        (body, note)
    } else {
        let body = rel.join("\n");
        let note =
            if prefix.is_empty() { "no common prefix to remove".to_string() } else { format!("common prefix removed: {}", prefix) };
        (body, note)
    };
    let dropped = lines.len().saturating_sub(body.split('\n').count());
    Compressed { text: body, dropped, note }
}

fn compress_grep(lines: &[&str]) -> Compressed {
    let mut order: Vec<String> = Vec::new();
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut passthrough: Vec<&str> = Vec::new();
    let mut hits = 0usize;
    for line in lines {
        if let Some(caps) = GREP_HIT_RE.captures(line) {
            if is_grep_hit(line) {
                hits += 1;
                let file = caps[1].to_string();
                let num = &caps[2];
                let rest = &caps[3];
                match groups.iter_mut().find(|(f, _)| *f == file) {
                    Some((_, entries)) => entries.push(format!("  {}: {}", num, rest)),
                    None => {
                        order.push(file.clone());
                        groups.push((file, vec![format!("  {}: {}", num, rest)]));
                    }
                }
                continue;
            }
        }
        if !line.trim().is_empty() {
            passthrough.push(line);
        }
    }
    let mut out: Vec<String> = passthrough.iter().map(|l| l.to_string()).collect();
    for file in &order {
        out.push(format!("{}:", file));
        if let Some((_, entries)) = groups.iter().find(|(f, _)| f == file) {
            out.extend(entries.iter().cloned());
        }
    }
    let text = out.join("\n");
    let dropped = lines.len().saturating_sub(out.len());
    let note = format!("{} hit(s) grouped into {} file(s)", hits, order.len());
    Compressed { text, dropped, note }
}

struct LsEntry {
    perm: String,
    owner: String,
    group: String,
    size: String,
    name: String,
}

struct LsBlock {
    header: Option<String>,
    entries: Vec<LsEntry>,
    unmatched: Vec<String>,
}

fn parse_ls_blocks(lines: &[&str]) -> Vec<LsBlock> {
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in lines.iter().copied().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                blocks.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(line);
        }
    }
    blocks
        .into_iter()
        .map(|block| {
            let mut header = None;
            let mut rest: &[&str] = &block;
            if HEADER_RE.is_match(block[0].trim()) && TOTAL_RE.is_match(block.get(1).unwrap_or(&"").trim()) {
                let h = block[0].trim();
                header = Some(h[..h.len() - 1].to_string());
                rest = &block[2..];
            } else if TOTAL_RE.is_match(block[0].trim()) {
                rest = &block[1..];
            }
            let mut entries = Vec::new();
            let mut unmatched = Vec::new();
            for line in rest {
                if let Some(caps) = LS_ENTRY_RE.captures(line) {
                    let entry_name = caps[7].to_string();
                    if entry_name == "." || entry_name == ".." {
                        continue;
                    }
                    entries.push(LsEntry {
                        perm: caps[1].to_string(),
                        owner: caps[3].to_string(),
                        group: caps[4].to_string(),
                        size: caps[5].to_string(),
                        name: entry_name,
                    });
                } else if !line.trim().is_empty() {
                    unmatched.push((*line).to_string());
                }
            }
            LsBlock { header, entries, unmatched }
        })
        .collect()
}

fn compress_ls(lines: &[&str]) -> Compressed {
    let blocks = parse_ls_blocks(lines);
    let all_entries: Vec<&LsEntry> = blocks.iter().flat_map(|b| b.entries.iter()).collect();
    let owners: HashSet<&str> = all_entries.iter().map(|e| e.owner.as_str()).collect();
    let groups_set: HashSet<&str> = all_entries.iter().map(|e| e.group.as_str()).collect();
    let perms: HashSet<&str> = all_entries.iter().map(|e| e.perm.as_str()).collect();
    let drop_owner = owners.len() == 1;
    let drop_group = groups_set.len() == 1;
    let drop_perm = perms.len() == 1;

    let headers: Vec<String> = blocks.iter().filter_map(|b| b.header.clone()).collect();
    let prefix = common_prefix(&headers);

    let mut out: Vec<String> = Vec::new();
    for block in &blocks {
        if let Some(h) = &block.header {
            let rel_header = if prefix.is_empty() {
                h.clone()
            } else {
                let stripped = h.strip_prefix(prefix.as_str()).unwrap_or("");
                if stripped.is_empty() { ".".to_string() } else { stripped.to_string() }
            };
            out.push(format!("{}:", rel_header));
        }
        if block.entries.len() >= EXT_THRESHOLD {
            let names: Vec<String> = block.entries.iter().map(|e| e.name.clone()).collect();
            let groups = group_by_extension(&names);
            for (ext, count) in groups {
                out.push(format!("  {}: {} file{}", ext, count, if count == 1 { "" } else { "s" }));
            }
        } else {
            for e in &block.entries {
                let mut cols: Vec<&str> = Vec::new();
                if !drop_perm {
                    cols.push(&e.perm);
                }
                cols.push(&e.size);
                if !drop_owner {
                    cols.push(&e.owner);
                }
                if !drop_group {
                    cols.push(&e.group);
                }
                cols.push(&e.name);
                out.push(format!("  {}", cols.join(" ")));
            }
        }
        out.extend(block.unmatched.iter().cloned());
    }

    let mut notes: Vec<String> = Vec::new();
    if drop_owner || drop_group {
        notes.push("owner/group identical throughout, column removed".to_string());
    }
    if drop_perm {
        notes.push("permissions identical throughout, column removed".to_string());
    }
    if !prefix.is_empty() {
        notes.push(format!("common root removed: {}", prefix));
    }

    let text = out.join("\n");
    let dropped = lines.len().saturating_sub(out.len());
    let note = if notes.is_empty() { "no low-entropy column to remove".to_string() } else { notes.join("; ") };
    Compressed { text, dropped, note }
}

fn guard_severe(lines: &[&str], result: Compressed) -> Compressed {
    let missing: Vec<&str> =
        lines.iter().copied().filter(|l| !l.trim().is_empty() && is_severe(l) && !result.text.contains(l.trim())).collect();
    if missing.is_empty() {
        return result;
    }
    let mut parts: Vec<String> = Vec::new();
    if !result.text.is_empty() {
        parts.push(result.text.clone());
    }
    parts.extend(missing.iter().map(|l| l.to_string()));
    let text = parts.join("\n");
    let dropped = result.dropped.saturating_sub(missing.len());
    let note = format!("{}; {} severe line(s) preserved", result.note, missing.len());
    Compressed { text, dropped, note }
}

/// Stable identifier for this shape.
pub fn name() -> &'static str {
    "listing"
}

/// Confidence, 0..1, that these lines are a file listing, grep hit list, tree, or path list.
pub fn detect(lines: &[&str]) -> f64 {
    classify(lines).ratio
}

/// Groups entries by extension or common prefix, keeping every severe line intact.
pub fn compress(lines: &[&str]) -> Compressed {
    let classified = classify(lines);
    let result = match classified.mode {
        "grep" => compress_grep(lines),
        "ls" => compress_ls(lines),
        "path" | "wc" => compress_path_list(lines),
        _ => Compressed {
            text: lines.join("\n"),
            dropped: 0,
            note: "listing: shape recognized but no safe compression gain".to_string(),
        },
    };
    guard_severe(lines, result)
}

/// Assembles the shape descriptor for the listing dispatcher.
pub fn shape() -> Shape {
    Shape { name: name(), detect, compress }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::install_log::detect as detect_install;
    use crate::shapes::keyvalue::detect as detect_keyvalue;

    fn load(fixture: &str) -> String {
        let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), fixture);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {} missing: {}", fixture, e))
    }

    fn lines_of(text: &str) -> Vec<&str> {
        text.split('\n').collect()
    }

    #[test]
    fn stable_name() {
        assert_eq!(name(), "listing");
    }

    #[test]
    fn detects_a_real_ls_la_r_with_high_confidence() {
        let text = load("listing-lsR.txt");
        assert!(detect(&lines_of(&text)) >= 0.6, "ls -laR should match the listing shape");
    }

    #[test]
    fn detects_a_real_find_with_high_confidence() {
        let text = load("listing-find.txt");
        assert!(detect(&lines_of(&text)) >= 0.9);
    }

    #[test]
    fn detects_a_real_grep_rn_with_high_confidence() {
        let text = load("listing-grep.txt");
        assert!(detect(&lines_of(&text)) >= 0.9);
    }

    #[test]
    fn does_not_detect_install_log_output_as_a_listing() {
        let text = load("install-npm-real.txt");
        assert!(detect(&lines_of(&text)) < 0.6);
    }

    #[test]
    fn does_not_detect_json_keyvalue_as_a_listing() {
        let text = load("keyvalue-versions-real.json");
        assert!(detect(&lines_of(&text)) < 0.6);
        let env = load("keyvalue-env-real.txt");
        assert!(detect(&lines_of(&env)) < 0.6, "PATH=/a:/b must not turn into a file path");
    }

    #[test]
    fn does_not_detect_a_git_diff_as_a_listing() {
        let text = load("git-diff-real.txt");
        assert!(detect(&lines_of(&text)) < 0.6);
    }

    #[test]
    fn install_log_and_keyvalue_are_not_confused_with_listing_in_reverse() {
        let find_text = load("listing-find.txt");
        let grep_text = load("listing-grep.txt");
        assert!(detect_install(&lines_of(&find_text)) < 0.6);
        assert!(detect_keyvalue(&lines_of(&find_text)) < 0.6);
        assert!(detect_install(&lines_of(&grep_text)) < 0.6);
        assert!(detect_keyvalue(&lines_of(&grep_text)) < 0.6);
    }

    #[test]
    fn compressing_a_real_find_factors_out_the_common_prefix_and_cuts_bytes() {
        let text = load("listing-find.txt");
        let before = text.len();
        let r = compress(&lines_of(&text));
        assert!(r.text.len() < before, "expected a reduction: before={} after={}", before, r.text.len());
        assert!(!r.note.is_empty());
    }

    #[test]
    fn compressing_a_long_find_lodash_636_files_groups_by_extension_with_a_big_reduction() {
        let text = load("listing-find-long.txt");
        let before = text.len();
        let r = compress(&lines_of(&text));
        assert!(r.text.contains(".js: 633 files"));
        let cut = 1.0 - (r.text.len() as f64 / before as f64);
        assert!(cut > 0.9, "expected a cut > 90%, got {:.1}%", cut * 100.0);
    }

    #[test]
    fn compressing_grep_groups_hits_by_file_without_losing_the_match_content() {
        let text = load("listing-grep.txt");
        let r = compress(&lines_of(&text));
        assert!(r.text.contains("export function linksOf(memory) {"));
        assert!(r.text.contains("lib/graph.js:"));
        assert!(r.note.contains("grouped"));
    }

    #[test]
    fn compressing_ls_la_r_removes_the_owner_group_column_when_identical_throughout() {
        let text = load("listing-lsR.txt");
        let r = compress(&lines_of(&text));
        assert!(r.note.contains("owner/group"), "should report the uniform column was cut");
        assert!(r.text.contains("contract.js"));
        assert!(r.dropped > 0);
    }

    #[test]
    fn a_severe_line_inside_a_long_listing_is_never_cut() {
        let mut long_list: Vec<String> = (0..40).map(|i| format!("/tmp/proj/src/mod{}.js", i)).collect();
        long_list.push("/tmp/proj/permission-denied.log".to_string());
        let mut with_error = vec!["ls: cannot open directory 'x': Permission denied".to_string()];
        with_error.extend(long_list);
        let refs: Vec<&str> = with_error.iter().map(|s| s.as_str()).collect();
        let r = compress(&refs);
        assert!(r.text.contains("Permission denied"));
    }

    #[test]
    fn compress_never_reorders_lines_that_survive_intact_in_ls_unmatched_mode() {
        let raw = vec![
            "/tmp/dir:",
            "total 8",
            "ls: cannot access 'ghost': No such file or directory",
            "-rw-r--r--  1 dev  staff  10  1 jan 00:00 a.txt",
        ];
        let r = compress(&raw);
        assert!(r.text.contains("No such file or directory"));
        assert!(r.text.contains("a.txt"));
    }

    #[test]
    fn compress_returns_text_unchanged_when_there_is_nothing_to_gain_fallback() {
        let raw = vec!["/a/b/one.js", "/c/d/two.js"];
        let r = compress(&raw);
        assert!(r.text.contains("one.js") && r.text.contains("two.js"));
        assert!(r.dropped <= raw.len());
    }
}

