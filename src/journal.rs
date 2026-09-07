use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

const KEEP_DAYS: i64 = 60;
const MAX_BODY: usize = 4000;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

pub fn default_path() -> PathBuf {
    home().join(".claude").join("bilro").join("journal.db")
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let db = Connection::open(path)?;
    db.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA journal_mode = WAL;
         CREATE VIRTUAL TABLE IF NOT EXISTS events USING fts5(
           kind, subject, body,
           session UNINDEXED, project UNINDEXED, at UNINDEXED
         );",
    )?;
    Ok(db)
}

pub fn open_default() -> rusqlite::Result<Connection> {
    open(&default_path())
}

/// The slug a project is filed under, matching how sessions are stored on disk.
pub fn project_of(cwd: &Path) -> String {
    cwd.to_string_lossy().replace('/', "-")
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}…")
}

/// Writes one thing worth remembering. Everything is redacted first: a prompt
/// or an error message is exactly where a pasted credential turns up, and this
/// table outlives the session that produced it.
pub fn record(
    db: &Connection,
    kind: &str,
    subject: &str,
    body: &str,
    session: &str,
    project: &str,
) -> rusqlite::Result<()> {
    let subject = crate::redact::redact(&clip(subject.trim(), 200));
    let body = crate::redact::redact(&clip(body.trim(), MAX_BODY));
    if subject.is_empty() && body.is_empty() {
        return Ok(());
    }
    db.execute(
        "INSERT INTO events(kind, subject, body, session, project, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![kind, subject, body, session, project, crate::ledger::now_ms()],
    )?;
    Ok(())
}

#[derive(Debug)]
pub struct Event {
    pub kind: String,
    pub subject: String,
    pub body: String,
    pub at: i64,
    pub session: String,
}

fn row_to_event(r: &rusqlite::Row) -> rusqlite::Result<Event> {
    Ok(Event {
        kind: r.get(0)?,
        subject: r.get(1)?,
        body: r.get(2)?,
        session: r.get(3)?,
        at: r.get::<_, i64>(4).or_else(|_| r.get::<_, String>(4).map(|v| v.parse().unwrap_or(0)))?,
    })
}

/// Ranked search. Scoped to one project by default, because recall is about
/// what happened here, not everywhere.
pub fn search(db: &Connection, query: &str, project: Option<&str>, limit: usize) -> rusqlite::Result<Vec<Event>> {
    let Some(match_query) = crate::sandbox::to_match_query(query) else {
        return Ok(Vec::new());
    };
    let mut sql = String::from(
        "SELECT kind, subject, body, session, at FROM events WHERE events MATCH ?1",
    );
    if project.is_some() {
        sql.push_str(" AND project = ?3");
    }
    sql.push_str(" ORDER BY bm25(events) LIMIT ?2");
    let mut st = db.prepare(&sql)?;
    let rows = match project {
        Some(p) => st.query_map(params![match_query, limit as i64, p], row_to_event)?.collect(),
        None => st.query_map(params![match_query, limit as i64], row_to_event)?.collect(),
    };
    rows
}

/// What happened, newest first, with no query at all. This is the view that
/// matters when a session resumes and the question is simply "where were we".
pub fn timeline(db: &Connection, project: Option<&str>, limit: usize) -> rusqlite::Result<Vec<Event>> {
    let mut sql = String::from("SELECT kind, subject, body, session, at FROM events");
    if project.is_some() {
        sql.push_str(" WHERE project = ?2");
    }
    sql.push_str(" ORDER BY CAST(at AS INTEGER) DESC LIMIT ?1");
    let mut st = db.prepare(&sql)?;
    match project {
        Some(p) => st.query_map(params![limit as i64, p], row_to_event)?.collect(),
        None => st.query_map(params![limit as i64], row_to_event)?.collect(),
    }
}

/// Drops what is old enough to have stopped mattering, so the file does not
/// grow without end.
pub fn prune(db: &Connection, older_than_days: i64) -> rusqlite::Result<usize> {
    let cutoff = crate::ledger::now_ms() - older_than_days * 86_400_000;
    let n = db.execute("DELETE FROM events WHERE CAST(at AS INTEGER) < ?1", params![cutoff])?;
    Ok(n)
}

pub fn prune_default(db: &Connection) -> rusqlite::Result<usize> {
    prune(db, KEEP_DAYS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "bilro-jr-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        open(&dir.join("j.db")).unwrap()
    }

    #[test]
    fn guarda_e_encontra_pelo_termo() {
        let d = db();
        record(&d, "prompt", "corrigir o spinner preso", "usuario pediu", "s1", "proj").unwrap();
        record(&d, "erro", "connection refused no redis", "porta 6379", "s1", "proj").unwrap();
        let hits = search(&d, "spinner", Some("proj"), 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].subject.contains("spinner"));
    }

    #[test]
    fn recall_e_por_projeto_nao_do_mundo_inteiro() {
        let d = db();
        record(&d, "prompt", "assunto compartilhado aqui", "x", "s1", "projeto-a").unwrap();
        record(&d, "prompt", "assunto compartilhado ali", "y", "s2", "projeto-b").unwrap();
        assert_eq!(search(&d, "compartilhado", Some("projeto-a"), 9).unwrap().len(), 1);
        assert_eq!(search(&d, "compartilhado", None, 9).unwrap().len(), 2);
    }

    #[test]
    fn linha_do_tempo_vem_do_mais_novo_para_o_mais_velho() {
        let d = db();
        for i in 0..5 {
            record(&d, "prompt", &format!("evento {i}"), "", "s1", "proj").unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let t = timeline(&d, Some("proj"), 3).unwrap();
        assert_eq!(t.len(), 3);
        assert!(t[0].subject.contains("evento 4"), "veio: {}", t[0].subject);
        assert!(t[0].at >= t[1].at);
    }

    #[test]
    fn segredo_colado_num_prompt_nao_fica_gravado() {
        let d = db();
        record(&d, "prompt", "usa esse banco", "DATABASE_URL=postgres://u:S3GR3DO@h:5432/d", "s1", "proj").unwrap();
        let t = timeline(&d, Some("proj"), 1).unwrap();
        assert!(!t[0].body.contains("S3GR3DO"), "gravou segredo: {}", t[0].body);
        assert!(t[0].body.contains("h:5432"), "mascarou demais");
    }

    #[test]
    fn evento_vazio_nao_e_gravado() {
        let d = db();
        record(&d, "prompt", "   ", "  ", "s1", "proj").unwrap();
        assert!(timeline(&d, Some("proj"), 5).unwrap().is_empty());
    }

    #[test]
    fn corpo_gigante_e_cortado_antes_de_gravar() {
        let d = db();
        record(&d, "erro", "estouro", &"x".repeat(50_000), "s1", "proj").unwrap();
        let t = timeline(&d, Some("proj"), 1).unwrap();
        assert!(t[0].body.chars().count() <= MAX_BODY + 1, "gravou {} chars", t[0].body.chars().count());
    }

    #[test]
    fn poda_remove_o_que_e_antigo_e_mantem_o_recente() {
        let d = db();
        record(&d, "prompt", "recente", "", "s1", "proj").unwrap();
        d.execute(
            "INSERT INTO events(kind, subject, body, session, project, at) VALUES ('prompt','antigo','','s0','proj',?1)",
            params![crate::ledger::now_ms() - 90 * 86_400_000i64],
        )
        .unwrap();
        assert_eq!(prune(&d, 60).unwrap(), 1);
        let t = timeline(&d, Some("proj"), 9).unwrap();
        assert_eq!(t.len(), 1);
        assert!(t[0].subject.contains("recente"));
    }

    #[test]
    fn aspas_na_busca_nao_quebram_a_consulta() {
        let d = db();
        record(&d, "erro", "falha no modulo auth", "", "s1", "proj").unwrap();
        for q in ["\"auth", "auth\" OR \"", "au*th", "'; DROP TABLE events; --"] {
            let _ = search(&d, q, Some("proj"), 5);
        }
        assert_eq!(timeline(&d, Some("proj"), 9).unwrap().len(), 1, "a tabela sobreviveu");
    }
}
