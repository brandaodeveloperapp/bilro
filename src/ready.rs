use crate::learn::{denoise, is_severe, open};
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

/// Every red-team run recorded so far, oldest first.
pub fn audits() -> Vec<Value> {
    std::fs::read_to_string(audits_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Value>>(&s).ok())
        .unwrap_or_default()
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

pub struct LostLine {
    pub sig: String,
    pub line: String,
}

pub struct Metrics {
    pub total: usize,
    pub learned: usize,
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

/// Replays every command bilro has learned and measures what it would do to
/// that output today. Coverage says how much of the real workload it can even
/// act on; the lost-line check is the invariant that matters most — a saver
/// that eats a failure line is worse than no saver at all.
pub fn evaluate(db: &Connection) -> rusqlite::Result<Metrics> {
    let mut stmt = db.prepare("SELECT r.sig, r.n, l.body FROM runs r LEFT JOIN last l ON l.sig = r.sig")?;
    let rows: Vec<(String, i64, Option<String>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    let total = rows.len();
    let mut learned = 0usize;
    let mut raw_bytes = 0usize;
    let mut kept_bytes = 0usize;
    let mut lost: Vec<LostLine> = Vec::new();

    for (sig, n, body) in &rows {
        let Some(body) = body else { continue };
        if body.is_empty() {
            continue;
        }
        if *n >= 3 {
            learned += 1;
        }
        let denoised = denoise(db, sig, body, 3, 0.8)?;
        raw_bytes += body.chars().count();
        kept_bytes += denoised.text.chars().count();
        let severe_in: Vec<&str> = body.split('\n').filter(|l| is_severe(l)).collect();
        if !severe_in.is_empty() {
            let out = &denoised.text;
            for line in severe_in {
                let trimmed = line.trim();
                if !out.contains(trimmed) {
                    lost.push(LostLine { sig: sig.clone(), line: trimmed.chars().take(80).collect() });
                }
            }
        }
    }

    let audits = audits();
    let criticals = audits.iter().map(|x| x.get("critical").and_then(|c| c.as_i64()).unwrap_or(0)).sum();
    let last_audit = audits.last().and_then(|x| x.get("at")).and_then(|a| a.as_u64());

    Ok(Metrics {
        total,
        learned,
        coverage: if total > 0 { learned as f64 / total as f64 } else { 0.0 },
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
            risk: "texto feio".to_string(),
            missing: why(&[("auditoria", audited)]),
        },
        Verdict {
            tool: "context-mode".to_string(),
            risk: "busca pior".to_string(),
            missing: why(&[("auditoria", audited), ("historico", enough)]),
        },
        Verdict {
            tool: "rtk".to_string(),
            risk: "output comido em silencio".to_string(),
            missing: why(&[
                ("auditoria", audited),
                ("historico", enough),
                ("cobertura", covered),
                ("economia", saving),
                ("sinal", safe),
            ]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learn::observe;
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
        let m = evaluate(&db()).unwrap();
        assert_eq!(m.total, 0);
        for v in verdicts(&m) {
            assert!(!v.missing.is_empty(), "{}", v.tool);
        }
    }

    #[test]
    fn cobertura_conta_so_comando_com_3_mais_execucoes() {
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
    fn perda_de_linha_de_falha_reprova_o_portao_do_rtk() {
        let mut d = db();
        for _ in 0..5 {
            observe(&mut d, "ci.sh", "FAIL auth\ntudo bem").unwrap();
        }
        let m = evaluate(&d).unwrap();
        assert_eq!(m.lost.len(), 0);
        assert!(!verdicts(&m).iter().find(|v| v.tool == "rtk").unwrap().missing.contains(&"sinal".to_string()));
    }
}

#[cfg(test)]
mod temp_manual_ready {
    use super::*;

    #[test]
    #[ignore]
    fn manual_ready_compare() {
        let m = evaluate_default().unwrap();
        eprintln!("total={}", m.total);
        eprintln!("learned={} coverage={:.4}", m.learned, m.coverage);
        eprintln!("savings={:.4}", m.savings);
        eprintln!("lost={}", m.lost.len());
        eprintln!("audits={} criticals={}", m.audits, m.criticals);
        eprintln!("last_audit={:?}", m.last_audit);
        for v in verdicts(&m) {
            eprintln!("{} missing={:?}", v.tool, v.missing);
        }
    }
}
