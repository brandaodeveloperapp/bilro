use once_cell::sync::Lazy;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

static VERSION: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bv?\d+\.\d+\.\d+\b").unwrap());
static BUILD: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(?:versionCode|build(?:Number)?)\s*:?\s*\d+").unwrap());
static MONEY: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$\s?[\d.,]+").unwrap());
static COUNT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b\d{2,}\s+(?:tenants?|users?|referrals?|commissions?|pods?)\b").unwrap()
});

/// One memory file: frontmatter fields plus the body below it.
pub struct Memory {
    pub path: PathBuf,
    pub name: String,
    pub kind: Option<String>,
    pub modified: Option<String>,
    pub verify: Option<String>,
    pub expect: Option<String>,
    pub body: String,
}

/// A claim the memory makes that can drift silently: version, build, money, count.
pub struct Claim {
    pub kind: &'static str,
    pub samples: Vec<String>,
}

/// One row of an audit pass over a set of memories.
pub struct AuditRow<'a> {
    pub memory: &'a Memory,
    pub status: &'static str,
    pub claims: Vec<Claim>,
    pub age: Option<i64>,
}

/// Knobs for `audit`: how old is stale, and what "now" means.
pub struct AuditOptions {
    pub stale_after_days: i64,
    pub now_ms: i64,
}

impl Default for AuditOptions {
    fn default() -> Self {
        Self { stale_after_days: 21, now_ms: now_ms() }
    }
}

/// Current time in epoch milliseconds, the unit every age comparison uses.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn frontmatter(text: &str) -> Option<(String, usize)> {
    let rest = text.strip_prefix("---\n")?;
    let idx = rest.find("\n---")?;
    let head = rest[..idx].to_string();
    Some((head, 4 + idx + 4))
}

fn field(head: &str, name: &str) -> Option<String> {
    for line in head.lines() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix(name) else { continue };
        let Some(rest) = rest.strip_prefix(':') else { continue };
        let val = rest.trim();
        if val.is_empty() {
            continue;
        }
        let val = val.strip_prefix(['"', '\'']).unwrap_or(val);
        let val = val.strip_suffix(['"', '\'']).unwrap_or(val);
        return Some(val.to_string());
    }
    None
}

/// Reads one memory file: frontmatter fields plus the body below it.
pub fn parse_memory(path: &Path) -> std::io::Result<Memory> {
    let text = fs::read_to_string(path)?;
    let (head, body) = match frontmatter(&text) {
        Some((head, end)) => (head, text[end..].to_string()),
        None => (String::new(), text.clone()),
    };
    let name = field(&head, "name").unwrap_or_else(|| {
        path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string()
    });
    Ok(Memory {
        path: path.to_path_buf(),
        name,
        kind: field(&head, "type"),
        modified: field(&head, "modified"),
        verify: field(&head, "verify"),
        expect: field(&head, "expect"),
        body,
    })
}

/// Every `.md` memory in a directory, skipping the index and backup copies.
pub fn load_memories(dir: &Path) -> Vec<Memory> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            name.ends_with(".md") && name != "MEMORY.md" && !name.contains(".bak-")
        })
        .collect();
    files.sort();
    files.into_iter().filter_map(|p| parse_memory(&p).ok()).collect()
}

fn collect_claims(kind: &'static str, re: &Regex, body: &str, out: &mut Vec<Claim>) {
    let mut samples: Vec<String> = Vec::new();
    for m in re.find_iter(body) {
        let s = m.as_str().to_string();
        if !samples.contains(&s) {
            samples.push(s);
        }
        if samples.len() == 4 {
            break;
        }
    }
    if !samples.is_empty() {
        out.push(Claim { kind, samples });
    }
}

/// Claims found in a memory's body: version, build, money, count patterns.
pub fn claims_in(memory: &Memory) -> Vec<Claim> {
    let mut out = Vec::new();
    collect_claims("version", &VERSION, &memory.body, &mut out);
    collect_claims("build", &BUILD, &memory.body, &mut out);
    collect_claims("money", &MONEY, &memory.body, &mut out);
    collect_claims("count", &COUNT, &memory.body, &mut out);
    out
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn parse_iso(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    if s.get(4..5)? != "-" {
        return None;
    }
    let month: i64 = s.get(5..7)?.parse().ok()?;
    if s.get(7..8)? != "-" {
        return None;
    }
    let day: i64 = s.get(8..10)?.parse().ok()?;
    if s.get(10..11)? != "T" {
        return None;
    }
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    if s.get(13..14)? != ":" {
        return None;
    }
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    if s.get(16..17)? != ":" {
        return None;
    }
    let second: i64 = s.get(17..19)?.parse().ok()?;
    let mut rest = &s[19..];
    let mut millis: i64 = 0;
    if let Some(frac_rest) = rest.strip_prefix('.') {
        let end = frac_rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(frac_rest.len());
        let mut frac = frac_rest[..end].to_string();
        while frac.len() < 3 {
            frac.push('0');
        }
        frac.truncate(3);
        millis = frac.parse().ok()?;
        rest = &frac_rest[end..];
    }
    if rest != "Z" {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some((days * 86400 + hour * 3600 + minute * 60 + second) * 1000 + millis)
}

/// Age of a memory in whole days, from its `modified` frontmatter field.
pub fn age_in_days(memory: &Memory, now_ms: i64) -> Option<i64> {
    let modified = memory.modified.as_ref()?;
    let t = parse_iso(modified)?;
    Some((now_ms - t).div_euclid(DAY_MS))
}

/// A memory is suspect when it asserts a fact that drifts, declares no way to
/// check itself, and has not been touched in a while.
pub fn audit(memories: &[Memory], opts: AuditOptions) -> Vec<AuditRow<'_>> {
    let mut out = Vec::new();
    for m in memories {
        let claims = claims_in(m);
        let age = age_in_days(m, opts.now_ms);
        if m.verify.is_some() {
            out.push(AuditRow { memory: m, status: "verifiable", claims, age });
        } else if !claims.is_empty() && age.map(|a| a >= opts.stale_after_days).unwrap_or(false) {
            out.push(AuditRow { memory: m, status: "suspect", claims, age });
        } else if !claims.is_empty() {
            out.push(AuditRow { memory: m, status: "asserts", claims, age });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn uniq_name() -> String {
        static N: AtomicU64 = AtomicU64::new(0);
        format!("t{}", N.fetch_add(1, Ordering::SeqCst))
    }

    fn vault(files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("bilro-mem-{}-{}", std::process::id(), uniq_name()));
        fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            fs::write(dir.join(name), body).unwrap();
        }
        dir
    }

    fn fm(extra: &str) -> String {
        format!("---\nname: test\ntype: project\nmodified: 2026-08-01T00:00:00.000Z\n{extra}---\n")
    }

    #[test]
    fn ignores_the_index_and_the_backups() {
        let dir = vault(&[
            ("MEMORY.md", "x"),
            ("a.md.bak-1", "y"),
            ("good.md", &format!("{}body", fm(""))),
        ]);
        assert_eq!(load_memories(&dir).len(), 1);
    }

    #[test]
    fn detects_version_money_and_count_as_a_claim() {
        let dir = vault(&[(
            "a.md",
            &format!("{}shipped 1.1.4 and cost $750 with 36 referrals", fm("")),
        )]);
        let memories = load_memories(&dir);
        let mut kinds: Vec<&str> = claims_in(&memories[0]).iter().map(|c| c.kind).collect();
        kinds.sort_unstable();
        assert_eq!(kinds, vec!["count", "money", "version"]);
    }

    #[test]
    fn text_with_no_aging_fact_does_not_become_suspect() {
        let dir = vault(&[(
            "a.md",
            &format!("{}always run the test before shipping", fm("")),
        )]);
        let memories = load_memories(&dir);
        assert_eq!(audit(&memories, AuditOptions::default()).len(), 0);
    }

    #[test]
    fn memory_with_verify_is_verifiable_not_suspect() {
        let dir = vault(&[(
            "a.md",
            &format!("{}version 1.1.8", fm("verify: echo 1.1.8\nexpect: 1.1.8\n")),
        )]);
        let memories = load_memories(&dir);
        let opts = AuditOptions { stale_after_days: 21, now_ms: parse_iso("2026-09-06T00:00:00Z").unwrap() };
        let rows = audit(&memories, opts);
        assert_eq!(rows[0].status, "verifiable");
    }

    #[test]
    fn old_claim_without_verify_becomes_suspect() {
        let dir = vault(&[("a.md", &format!("{}the version is 1.1.4", fm("")))]);
        let memories = load_memories(&dir);
        let opts = AuditOptions { stale_after_days: 21, now_ms: parse_iso("2026-09-06T00:00:00Z").unwrap() };
        let rows = audit(&memories, opts);
        assert_eq!(rows[0].status, "suspect");
    }

    #[test]
    fn recent_claim_is_not_suspect_yet() {
        let dir = vault(&[("a.md", &format!("{}the version is 1.1.4", fm("")))]);
        let memories = load_memories(&dir);
        let opts = AuditOptions { stale_after_days: 21, now_ms: parse_iso("2026-08-03T00:00:00Z").unwrap() };
        let rows = audit(&memories, opts);
        assert_eq!(rows[0].status, "asserts");
    }

    #[test]
    fn age_in_days_comes_from_frontmatter() {
        let dir = vault(&[("a.md", &format!("{}1.0.0", fm("")))]);
        let memories = load_memories(&dir);
        let now = parse_iso("2026-08-11T00:00:00Z").unwrap();
        assert_eq!(age_in_days(&memories[0], now), Some(10));
    }
}
