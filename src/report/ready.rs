use crate::compress::learn::{denoise, open};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::PathBuf;

pub const GATE_COVERAGE: f64 = 0.8;
pub const GATE_MIN_AUDITS: i64 = 2;
pub const GATE_MAX_CRITICALS: i64 = 0;
pub const GATE_SAVINGS: f64 = 0.5;
pub const GATE_MIN_COMMANDS: i64 = 40;

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

fn bilro_home() -> PathBuf {
    home_dir().join(".claude").join("bilro")
}

fn audits_path() -> PathBuf {
    bilro_home().join("audits.json")
}

/// A record of red-team runs is a record, not a lock: anything with a shell can
/// rewrite this file, and bilro hands out shells for a living. The cap is here
/// so a corrupt or oversized file cannot stall the gate, not to keep anyone out.
const MAX_AUDITS_BYTES: u64 = 1024 * 1024;

/// Every red-team run recorded so far, oldest first.
pub fn audits() -> Vec<Value> {
    let path = audits_path();
    if std::fs::metadata(&path).map(|m| m.len() > MAX_AUDITS_BYTES).unwrap_or(false) {
        return Vec::new();
    }
    let parsed: Vec<Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        .unwrap_or_default();
    if parsed.iter().any(|e| !counts_are_whole_numbers(e)) {
        return Vec::new();
    }
    parsed
}

/// A count that is not a whole number is not a count. Coercing `"3"` or `2.0`
/// to zero turned five recorded criticals into a green gate, so a file with one
/// of those in it is treated as unreadable rather than partly believed.
fn counts_are_whole_numbers(entry: &Value) -> bool {
    ["critical", "high", "medium", "low"].iter().all(|k| match entry.get(k) {
        None => true,
        Some(v) => v.as_i64().is_some_and(|n| n >= 0),
    })
}

/// Text read back from the audit file reaches a terminal. An escape sequence in
/// a note could clear the screen and take the verdict with it, and a
/// right-to-left override could reverse it in place without deleting a thing.
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            let bidi = matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2069}' | '\u{feff}' | '\u{3000}');
            if (c.is_control() && c != '\t') || bidi {
                ' '
            } else {
                c
            }
        })
        .take(400)
        .collect()
}

/// Appends one audit entry, stamped with the current time, and persists it.
pub fn record_audit(entry: Value) -> std::io::Result<Vec<Value>> {
    let mut all = audits();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut merged = json!({ "at": at });
    if let (Value::Object(m), Value::Object(e)) = (&mut merged, &entry) {
        for (k, v) in e {
            m.insert(k.clone(), v.clone());
        }
    }
    all.push(merged);
    std::fs::create_dir_all(bilro_home())?;
    std::fs::write(audits_path(), serde_json::to_string_pretty(&all)?)?;
    Ok(all)
}

/// Whether a line's content is still reachable in the output. Asking for the
/// line verbatim marks a shape that re-flows `modified: path` into
/// `modified (17): path, path, path` as having destroyed seventeen lines, when
/// every path is still right there. What must survive is the content, not the
/// layout — so the question is whether ALL of the line's own words are still
/// findable. Asking for any one of them let `{"name":"api-5","status":"errored"}`
/// count as surviving because another item still said `"name":`, and a line
/// made only of short words counted as surviving even against empty output.
/// Punctuation is trimmed off each word first: a shape that re-flows
/// `modified:   path` into `modified (17): path` did not destroy the word
/// `modified`, it moved the colon.
fn content_survives(line: &str, out: &str) -> bool {
    if out.contains(line) {
        return true;
    }
    let mut had_a_word = false;
    for token in line.split_whitespace() {
        let core = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '.' && c != '-' && c != '_');
        if core.chars().count() < 6 {
            continue;
        }
        had_a_word = true;
        if !out.contains(core) {
            return false;
        }
    }
    had_a_word
}

pub struct LostLine {
    pub sig: String,
    pub line: String,
}

pub struct Metrics {
    pub total: usize,
    pub learned: usize,
    pub volume: usize,
    pub volume_learned: usize,
    pub coverage: f64,
    pub savings: f64,
    pub lost: Vec<LostLine>,
    pub audits: usize,
    pub criticals: i64,
    pub last_audit: Option<u64>,
}

fn default_db() -> rusqlite::Result<Connection> {
    open(&home_dir().join(".claude").join("bilro").join("learn.db"))
}

/// Replays every command bilro has learned through the WHOLE pipeline — shape
/// first, then denoise, exactly as production runs it — and measures what it
/// would do to that output today. Measuring denoise alone left every line a
/// shape removes outside the gate.
///
/// Coverage says how much of the real workload it can even act on, weighted by
/// the output volume each command actually produced — counting distinct
/// signatures instead would let a one-shot heredoc that printed two lines
/// outweigh a build log seen fifty times. The volume is what `observe` actually
/// accumulated, run by run; multiplying the last body by the run count let one
/// freak 400 KB run stand in for the forty-nine quiet ones before it.
///
/// Production redacts before it compresses, so the replay does too — a masked
/// credential is the tool working, and counting it as a lost line would bury
/// the real ones.
///
/// The lost-line check is the invariant that matters most: no line reporting a
/// failure may leave the pipeline. Asking that of `denoise` alone was a
/// question that could not fail, because `denoise` keeps every `is_severe` line
/// by construction — and for two rounds of review it never did fail. Asking it
/// of the WHOLE pipeline is a different question with real teeth: a shape has
/// no such rule, and collapsing an array really did delete the one container
/// that had died.
///
/// Counting every vanished line instead, and forgiving as many as the pipeline
/// declared, was tried and abandoned: a shape that drops the owner column of a
/// listing announces it in prose, not in a number, so the count read a
/// faithful reformat as mass deletion.
pub fn evaluate(db: &Connection) -> rusqlite::Result<Metrics> {
    evaluate_with(db, &audits())
}

/// Same measurement against a given audit history, so a test can state the
/// history it means instead of reading whatever this machine happens to hold.
pub fn evaluate_with(db: &Connection, history: &[Value]) -> rusqlite::Result<Metrics> {
    let mut stmt = db.prepare(
        "SELECT r.sig, r.n, r.bytes, l.body FROM runs r LEFT JOIN last l ON l.sig = r.sig",
    )?;
    let rows: Vec<(String, i64, i64, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    let total = rows.len();
    let mut learned = 0usize;
    let mut raw_bytes = 0usize;
    let mut kept_bytes = 0usize;
    let mut volume_total = 0usize;
    let mut volume_seen = 0usize;
    let mut lost: Vec<LostLine> = Vec::new();

    for (sig, n, seen, body) in &rows {
        let Some(body) = body else { continue };
        if body.is_empty() {
            continue;
        }
        let volume = if *seen > 0 { *seen as usize } else { *n as usize * body.chars().count() };
        volume_total += volume;
        if *n >= 3 {
            learned += 1;
            volume_seen += volume;
        }
        let body = &crate::redact::redact(body);
        let shaped = crate::compress::shapes::apply(&crate::compress::pipeline::all_shapes(), body);
        let denoised = denoise(db, sig, &shaped.text, 3, 0.8)?;
        let out = &denoised.text;
        raw_bytes += body.chars().count();
        kept_bytes += out.chars().count();

        for line in body.split('\n').map(str::trim) {
            if line.is_empty() {
                continue;
            }
            let Some(evidence) = crate::compress::learn::severe_evidence(line) else { continue };
            if !out.contains(evidence.as_str()) && !content_survives(line, out) {
                lost.push(LostLine { sig: sig.clone(), line: line.chars().take(80).collect() });
            }
        }
    }

    let audits = history.to_vec();
    let criticals = audits.iter().map(|x| x.get("critical").and_then(|c| c.as_i64()).unwrap_or(0)).sum();
    let last_audit = audits.last().and_then(|x| x.get("at")).and_then(|a| a.as_u64());

    Ok(Metrics {
        total,
        learned,
        volume: volume_total,
        volume_learned: volume_seen,
        coverage: if volume_total > 0 { volume_seen as f64 / volume_total as f64 } else { 0.0 },
        savings: if raw_bytes > 0 { 1.0 - kept_bytes as f64 / raw_bytes as f64 } else { 0.0 },
        lost,
        audits: audits.len(),
        criticals,
        last_audit,
    })
}

/// Evaluates against the default `learn.db` under `~/.claude/bilro`.
pub fn evaluate_default() -> rusqlite::Result<Metrics> {
    let db = default_db()?;
    evaluate(&db)
}

pub struct Verdict {
    pub tool: String,
    pub risk: String,
    pub missing: Vec<String>,
}

fn why(pairs: &[(&str, bool)]) -> Vec<String> {
    pairs.iter().filter(|(_, ok)| !ok).map(|(name, _)| name.to_string()).collect()
}

/// One verdict per tool, ordered by how much damage a wrong swap would do.
pub fn verdicts(m: &Metrics) -> Vec<Verdict> {
    let enough = m.total as i64 >= GATE_MIN_COMMANDS;
    let safe = m.lost.is_empty();
    let audited = m.audits as i64 >= GATE_MIN_AUDITS && m.criticals <= GATE_MAX_CRITICALS;
    let covered = m.coverage >= GATE_COVERAGE;
    let saving = m.savings >= GATE_SAVINGS;

    vec![
        Verdict {
            tool: "caveman".to_string(),
            risk: "ugly prose".to_string(),
            missing: why(&[("audit", audited)]),
        },
        Verdict {
            tool: "context-mode".to_string(),
            risk: "worse search".to_string(),
            missing: why(&[("audit", audited), ("history", enough)]),
        },
        Verdict {
            tool: "rtk".to_string(),
            risk: "output eaten in silence".to_string(),
            missing: why(&[
                ("audit", audited),
                ("history", enough),
                ("coverage", covered),
                ("savings", saving),
                ("signal", safe),
            ]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compress::learn::observe;
    use std::path::Path;

    fn db() -> Connection {
        let dir = std::env::temp_dir().join(format!("bilro-ready-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("{}.db", uniq_name()));
        open(Path::new(&file)).unwrap()
    }

    fn uniq_name() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        format!("t{}", N.fetch_add(1, Ordering::SeqCst))
    }

    #[test]
    fn banco_vazio_nao_libera_nada() {
        let m = evaluate_with(&db(), &[]).unwrap();
        assert_eq!(m.total, 0);
        for v in verdicts(&m) {
            assert!(!v.missing.is_empty(), "{}", v.tool);
        }
    }

    #[test]
    fn coverage_counts_only_commands_with_three_or_more_runs() {
        let mut d = db();
        for i in 0..5 {
            observe(&mut d, "npm test", &format!("ok {i}\nsempre igual")).unwrap();
        }
        observe(&mut d, "npm run build", "rodou uma vez").unwrap();
        let m = evaluate(&d).unwrap();
        assert_eq!(m.total, 2);
        assert_eq!(m.learned, 1);
    }

    #[test]
    fn a_one_shot_command_does_not_crater_coverage() {
        let mut d = db();
        for _ in 0..10 {
            observe(&mut d, "cargo build", &"warning: unused import\n".repeat(40)).unwrap();
        }
        for i in 0..20 {
            observe(&mut d, &format!("sed -i s/a/b/ file{i}.txt"), "done").unwrap();
        }
        let m = evaluate(&d).unwrap();
        assert_eq!(m.total, 21);
        assert_eq!(m.learned, 1);
        assert!(
            m.coverage > 0.9,
            "one big repeated command is the workload, not twenty tiny one-shots: {}",
            m.coverage
        );
    }

    #[test]
    fn coverage_is_volume_not_headcount() {
        let mut d = db();
        for _ in 0..5 {
            observe(&mut d, "npm test", "ok\n").unwrap();
        }
        observe(&mut d, "huge one-shot", &"x\n".repeat(10_000)).unwrap();
        let m = evaluate(&d).unwrap();
        assert_eq!(m.learned, 1);
        assert_eq!(m.total, 2);
        assert!(m.coverage < 0.1, "unlearned bulk must dominate: {}", m.coverage);
    }

    #[test]
    fn an_empty_database_has_no_coverage_and_does_not_divide_by_zero() {
        let m = evaluate(&db()).unwrap();
        assert_eq!(m.volume, 0);
        assert_eq!(m.coverage, 0.0);
    }

    #[test]
    fn perda_de_linha_de_falha_reprova_o_portao_do_rtk() {
        let mut d = db();
        for _ in 0..5 {
            observe(&mut d, "ci.sh", "FAIL auth\ntudo bem").unwrap();
        }
        let m = evaluate(&d).unwrap();
        assert_eq!(m.lost.len(), 0);
        assert!(!verdicts(&m).iter().find(|v| v.tool == "rtk").unwrap().missing.contains(&"signal".to_string()));
    }
}

#[cfg(test)]
mod audit_gate_tests {
    use super::*;

    fn clean(by: &str) -> Value {
        json!({ "at": 1u64, "by": by, "critical": 0, "high": 0 })
    }

    #[test]
    fn two_clean_red_teams_release_caveman() {
        let d = {
            let dir = std::env::temp_dir().join("bilro-audit-gate");
            let _ = std::fs::create_dir_all(&dir);
            open(&dir.join("a.db")).unwrap()
        };
        let m = evaluate_with(&d, &[clean("security"), clean("correctness")]).unwrap();
        let v = verdicts(&m);
        assert!(v.iter().find(|x| x.tool == "caveman").unwrap().missing.is_empty());
    }

    #[test]
    fn one_red_team_is_not_enough() {
        let d = {
            let dir = std::env::temp_dir().join("bilro-audit-gate");
            let _ = std::fs::create_dir_all(&dir);
            open(&dir.join("b.db")).unwrap()
        };
        let m = evaluate_with(&d, &[clean("security")]).unwrap();
        assert!(verdicts(&m).iter().all(|v| v.missing.contains(&"audit".to_string())));
    }

    #[test]
    fn a_single_critical_holds_every_tool_back() {
        let d = {
            let dir = std::env::temp_dir().join("bilro-audit-gate");
            let _ = std::fs::create_dir_all(&dir);
            open(&dir.join("c.db")).unwrap()
        };
        let mut bad = clean("security");
        bad["critical"] = json!(1);
        let m = evaluate_with(&d, &[bad, clean("correctness")]).unwrap();
        assert_eq!(m.criticals, 1);
        assert!(verdicts(&m).iter().all(|v| v.missing.contains(&"audit".to_string())));
    }
}

#[cfg(test)]
mod audit_file_tests {
    use super::*;

    #[test]
    fn a_note_cannot_wipe_the_verdict_off_the_screen() {
        let hidden = printable("clean\u{1b}[2J\u{1b}[H3 critical still open");
        assert!(!hidden.contains('\u{1b}'));
        assert!(hidden.contains("3 critical still open"));
    }

    #[test]
    fn a_note_cannot_scroll_the_verdict_away() {
        assert_eq!(printable(&"x\n".repeat(500)).chars().count(), 400);
    }
}

#[cfg(test)]
mod audit_integrity_tests {

    #[test]
    fn the_check_can_still_fail_when_the_evidence_really_goes() {
        use crate::compress::learn::severe_evidence;
        let dropped = "web-7 0/1 CrashLoopBackOff 47 restarts";
        let out_without_it = "web-0 1/1 Running\nweb-1 1/1 Running";
        assert_eq!(severe_evidence(dropped).as_deref(), Some("CrashLoopBackOff"));
        assert!(!out_without_it.contains("CrashLoopBackOff"));
        assert!(!content_survives(dropped, out_without_it), "the oracle would forgive a real deletion");
    }

    #[test]
    fn a_reformat_that_keeps_the_evidence_is_not_a_loss() {
        use crate::compress::learn::severe_evidence;
        let line = "{\"name\":\"api-5\",\"status\":\"errored\",\"restarts\":12}";
        let collapsed = "{ \"(example)\": {...}, \"(failures)\": [{\"name\":\"api-5\",\"status\":\"errored\"}] }";
        let evidence = severe_evidence(line).unwrap();
        assert!(collapsed.contains(evidence.as_str()), "evidence {evidence} not in {collapsed}");
    }
    use super::*;

    #[test]
    fn a_count_that_is_not_a_whole_number_makes_the_record_unreadable() {
        assert!(!counts_are_whole_numbers(&json!({ "critical": "3" })));
        assert!(!counts_are_whole_numbers(&json!({ "critical": 2.0 })));
        assert!(!counts_are_whole_numbers(&json!({ "high": -1 })));
        assert!(!counts_are_whole_numbers(&json!({ "critical": null })));
        assert!(counts_are_whole_numbers(&json!({ "critical": 0, "high": 5 })));
        assert!(counts_are_whole_numbers(&json!({ "by": "x" })));
    }

    #[test]
    fn a_note_cannot_reverse_the_verdict_in_place() {
        let out = printable("clean\u{202e}0 lacitirc 3");
        assert!(!out.contains('\u{202e}'));
    }
}
