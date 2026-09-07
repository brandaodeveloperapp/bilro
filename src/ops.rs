use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

fn bilro_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("bilro")
}

fn learn_db_path(home: &Path) -> PathBuf {
    bilro_dir(home).join("learn.db")
}

fn index_db_path(home: &Path) -> PathBuf {
    bilro_dir(home).join("index.db")
}

fn journal_db_path(home: &Path) -> PathBuf {
    bilro_dir(home).join("journal.db")
}

fn sessions_dir_path(home: &Path) -> PathBuf {
    bilro_dir(home).join("sessions")
}

fn settings_path(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

fn claude_json_path(home: &Path) -> PathBuf {
    home.join(".claude.json")
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Session files bilro owns under a sessions directory, ignoring the lock
/// files a concurrent write leaves behind.
fn session_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect()
}

fn total_bytes(paths: &[PathBuf]) -> u64 {
    paths.iter().map(|p| file_len(p)).sum()
}

fn count_table(db: &Connection, table: &str) -> usize {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
}

/// What bilro has learned so far, and what a denoise pass would cut today.
#[derive(Debug)]
pub struct Stats {
    pub learned_commands: usize,
    pub learned_mature: usize,
    pub denoise_savings_bytes: u64,
    pub indexed_chunks: usize,
    pub journal_events_total: usize,
    pub journal_events_by_kind: Vec<(String, usize)>,
    pub sessions: usize,
    pub learn_db_bytes: u64,
    pub index_db_bytes: u64,
    pub journal_db_bytes: u64,
    pub sessions_bytes: u64,
}

/// Replays `denoise` over every command shape's last known body, so the
/// savings figure is measured against the real corpus instead of guessed.
fn measure_denoise_savings(db: &Connection) -> u64 {
    let Ok(mut stmt) = db.prepare("SELECT sig, body FROM last") else { return 0 };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else {
        return 0;
    };
    rows.flatten()
        .filter_map(|(sig, body)| {
            crate::learn::denoise(db, &sig, &body, 3, 0.8)
                .ok()
                .map(|d| body.len().saturating_sub(d.text.len()) as u64)
        })
        .sum()
}

fn stats_at(home: &Path) -> Stats {
    let learn_path = learn_db_path(home);
    let (learned_commands, learned_mature, denoise_savings_bytes) = learn_path
        .exists()
        .then(|| crate::learn::open(&learn_path).ok())
        .flatten()
        .map(|db| {
            let total: i64 = db.query_row("SELECT count(*) FROM runs", [], |r| r.get(0)).unwrap_or(0);
            let mature: i64 =
                db.query_row("SELECT count(*) FROM runs WHERE n >= 3", [], |r| r.get(0)).unwrap_or(0);
            (total as usize, mature as usize, measure_denoise_savings(&db))
        })
        .unwrap_or((0, 0, 0));

    let index_path = index_db_path(home);
    let indexed_chunks = index_path
        .exists()
        .then(|| crate::sandbox::open(&index_path).ok())
        .flatten()
        .and_then(|db| db.query_row("SELECT count(*) FROM chunks", [], |r| r.get::<_, i64>(0)).ok())
        .unwrap_or(0) as usize;

    let journal_path = journal_db_path(home);
    let (journal_events_total, journal_events_by_kind) = journal_path
        .exists()
        .then(|| crate::journal::open(&journal_path).ok())
        .flatten()
        .map(|db| {
            let total: i64 = db.query_row("SELECT count(*) FROM events", [], |r| r.get(0)).unwrap_or(0);
            let mut by_kind = Vec::new();
            if let Ok(mut st) = db.prepare("SELECT kind, count(*) FROM events GROUP BY kind ORDER BY kind") {
                if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))) {
                    by_kind.extend(rows.flatten());
                }
            }
            (total as usize, by_kind)
        })
        .unwrap_or((0, Vec::new()));

    let sessions = session_files(&sessions_dir_path(home));

    Stats {
        learned_commands,
        learned_mature,
        denoise_savings_bytes,
        indexed_chunks,
        journal_events_total,
        journal_events_by_kind,
        sessions: sessions.len(),
        learn_db_bytes: file_len(&learn_path),
        index_db_bytes: file_len(&index_path),
        journal_db_bytes: file_len(&journal_path),
        sessions_bytes: total_bytes(&sessions),
    }
}

/// How much context bilro has saved so far, measured against what it holds
/// right now: learned command shapes, the chunk index, the journal, sessions.
pub fn stats() -> Stats {
    stats_at(&home())
}

/// One diagnostic result. A failing check always says how to fix it.
#[derive(Debug)]
pub struct Check {
    pub nome: String,
    pub ok: bool,
    pub detalhe: String,
}

fn check(nome: &str, ok: bool, detalhe: impl Into<String>) -> Check {
    Check { nome: nome.to_string(), ok, detalhe: detalhe.into() }
}

fn which(program: &str) -> Option<PathBuf> {
    let out = Command::new("which").arg(program).stdout(Stdio::piped()).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!found.is_empty()).then(|| PathBuf::from(found))
}

fn check_binary_on_path() -> Check {
    let Ok(current) = std::env::current_exe() else {
        return check(
            "binario no PATH",
            false,
            "std::env::current_exe() falhou. Rode o binario pelo caminho absoluto para diagnosticar.",
        );
    };
    let Some(found) = which("bilro") else {
        return check(
            "binario no PATH",
            false,
            format!(
                "`bilro` nao esta no PATH. Rode `{} install` ou crie um symlink num diretorio do PATH.",
                current.display()
            ),
        );
    };
    let same = std::fs::canonicalize(&found).ok() == std::fs::canonicalize(&current).ok();
    if same {
        check("binario no PATH", true, format!("{} -> {}", found.display(), current.display()))
    } else {
        check(
            "binario no PATH",
            false,
            format!(
                "`bilro` no PATH aponta para {}, mas o executavel atual e {}. Rode `bilro install` de novo para atualizar o symlink.",
                found.display(),
                current.display()
            ),
        )
    }
}

fn check_hooks(home: &Path) -> Check {
    let path = settings_path(home);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return check(
            "hooks em settings.json",
            false,
            format!("nao achei {}. Rode `bilro install` para registrar os hooks.", path.display()),
        );
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return check(
            "hooks em settings.json",
            false,
            format!("{} nao e JSON valido. Corrija-o ou apague e rode `bilro install` de novo.", path.display()),
        );
    };
    let is_bilro = |cmd: &str| -> bool {
        cmd.split_whitespace()
            .next()
            .and_then(|bin| Path::new(bin).file_name())
            .and_then(|f| f.to_str())
            == Some("bilro")
    };

    let commands: Vec<String> = value
        .get("hooks")
        .and_then(|h| h.as_object())
        .into_iter()
        .flat_map(|groups| groups.values())
        .filter_map(|list| list.as_array())
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(|h| h.as_array()))
        .flatten()
        .filter_map(|h| h.get("command").and_then(|c| c.as_str()))
        .map(str::to_string)
        .filter(|cmd| is_bilro(cmd))
        .collect();

    if commands.is_empty() {
        return check(
            "hooks em settings.json",
            false,
            format!("nenhum hook do bilro registrado em {}. Rode `bilro install`.", path.display()),
        );
    }
    let faltando: Vec<&String> = commands
        .iter()
        .filter(|cmd| {
            let bin = cmd.split_whitespace().next().unwrap_or("");
            bin.is_empty() || !Path::new(bin).exists()
        })
        .collect();
    if faltando.is_empty() {
        check("hooks em settings.json", true, format!("{} hook(s), todos com binario existente", commands.len()))
    } else {
        check(
            "hooks em settings.json",
            false,
            format!(
                "hook(s) apontando para binario que sumiu: {faltando:?}. Rode `bilro install` de novo para corrigir o caminho."
            ),
        )
    }
}

fn check_mcp(home: &Path) -> Check {
    let path = claude_json_path(home);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return check("servidor MCP em .claude.json", false, format!("nao achei {}. Rode `bilro install`.", path.display()));
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return check("servidor MCP em .claude.json", false, format!("{} nao e JSON valido.", path.display()));
    };
    let Some(cmd) = value
        .get("mcpServers")
        .and_then(|m| m.get("bilro"))
        .and_then(|b| b.get("command"))
        .and_then(|c| c.as_str())
    else {
        return check(
            "servidor MCP em .claude.json",
            false,
            "mcpServers.bilro nao registrado ou sem \"command\". Rode `bilro install`.",
        );
    };
    if Path::new(cmd).exists() {
        check("servidor MCP em .claude.json", true, format!("aponta para {cmd}"))
    } else {
        check(
            "servidor MCP em .claude.json",
            false,
            format!("aponta para {cmd}, que nao existe mais. Rode `bilro install` de novo."),
        )
    }
}

fn check_db(nome: &str, path: &Path, opener: impl Fn(&Path) -> rusqlite::Result<Connection>) -> Check {
    match opener(path) {
        Ok(_) => check(nome, true, format!("abre em {}", path.display())),
        Err(e) => check(
            nome,
            false,
            format!(
                "{} nao abre ({e}). Se o arquivo estiver corrompido, mova-o para outro nome e deixe o bilro recriar.",
                path.display()
            ),
        ),
    }
}

fn check_fts5() -> Check {
    match Connection::open_in_memory() {
        Ok(conn) => match conn.execute_batch("CREATE VIRTUAL TABLE bilro_doctor_probe USING fts5(x);") {
            Ok(()) => check("FTS5 disponivel", true, "CREATE VIRTUAL TABLE ... USING fts5 funcionou em memoria"),
            Err(e) => check(
                "FTS5 disponivel",
                false,
                format!(
                    "sqlite sem FTS5 ({e}). Recompile com a feature \"bundled\" do rusqlite (ja e a default deste projeto)."
                ),
            ),
        },
        Err(e) => check("FTS5 disponivel", false, format!("nao consegui abrir sqlite em memoria: {e}")),
    }
}

fn check_scripts() -> Check {
    let faltando: Vec<&'static str> = crate::script::languages()
        .into_iter()
        .filter_map(crate::script::runtime_for)
        .filter(|rt| !crate::script::available(rt))
        .map(|rt| rt.program)
        .collect();
    if faltando.is_empty() {
        check(
            "interpretadores do script",
            true,
            format!("disponiveis: {}", crate::script::languages().join(", ")),
        )
    } else {
        check(
            "interpretadores do script",
            false,
            format!("faltam: {}. Instale-os para essas linguagens funcionarem em `bilro run`.", faltando.join(", ")),
        )
    }
}

fn check_curl() -> Check {
    match which("curl") {
        Some(p) => check("curl presente", true, format!("{}", p.display())),
        None => check("curl presente", false, "curl nao esta no PATH. Instale curl, usado pelo fetch."),
    }
}

fn check_disk_usage(home: &Path) -> Check {
    let learn = file_len(&learn_db_path(home));
    let index = file_len(&index_db_path(home));
    let journal = file_len(&journal_db_path(home));
    let sessions = total_bytes(&session_files(&sessions_dir_path(home)));
    let total = learn + index + journal + sessions;
    check(
        "espaco em disco dos bancos",
        true,
        format!(
            "total {total} bytes (learn.db: {learn}, index.db: {index}, journal.db: {journal}, sessions: {sessions}). Acima de dezenas de MB, rode `bilro purge`."
        ),
    )
}

fn doctor_at(home: &Path) -> Vec<Check> {
    vec![
        check_binary_on_path(),
        check_hooks(home),
        check_mcp(home),
        check_db("banco learn.db abre", &learn_db_path(home), crate::learn::open),
        check_db("banco index.db abre", &index_db_path(home), crate::sandbox::open),
        check_db("banco journal.db abre", &journal_db_path(home), crate::journal::open),
        check_fts5(),
        check_scripts(),
        check_curl(),
        check_disk_usage(home),
    ]
}

/// Every self-check bilro can run: installation, indexes, and the external
/// tools it shells out to.
pub fn doctor() -> Vec<Check> {
    doctor_at(&home())
}

/// What to erase. `All` clears every table bilro keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purge {
    Index,
    Journal,
    Learn,
    Sessions,
    All,
}

/// Rows and bytes one purge target held before (and whether it was actually
/// cleared, which only happens when the caller confirmed).
#[derive(Debug)]
pub struct TargetReport {
    pub name: String,
    pub rows: usize,
    pub bytes: u64,
    pub purged: bool,
}

#[derive(Debug)]
pub struct PurgeReport {
    pub confirmed: bool,
    pub targets: Vec<TargetReport>,
}

fn purge_index_at(home: &Path, confirmed: bool) -> TargetReport {
    let path = index_db_path(home);
    let bytes = file_len(&path);
    let db = path.exists().then(|| crate::sandbox::open(&path).ok()).flatten();
    let rows = db.as_ref().map(|d| count_table(d, "chunks")).unwrap_or(0);
    if confirmed {
        if let Some(d) = &db {
            let _ = d.execute("DELETE FROM chunks", []);
            let _ = d.execute_batch("VACUUM;");
        }
    }
    TargetReport { name: "index".to_string(), rows, bytes, purged: confirmed }
}

fn purge_journal_at(home: &Path, confirmed: bool) -> TargetReport {
    let path = journal_db_path(home);
    let bytes = file_len(&path);
    let db = path.exists().then(|| crate::journal::open(&path).ok()).flatten();
    let rows = db.as_ref().map(|d| count_table(d, "events")).unwrap_or(0);
    if confirmed {
        if let Some(d) = &db {
            let _ = d.execute("DELETE FROM events", []);
            let _ = d.execute_batch("VACUUM;");
        }
    }
    TargetReport { name: "journal".to_string(), rows, bytes, purged: confirmed }
}

fn purge_learn_at(home: &Path, confirmed: bool) -> TargetReport {
    let path = learn_db_path(home);
    let bytes = file_len(&path);
    let db = path.exists().then(|| crate::learn::open(&path).ok()).flatten();
    let rows = db
        .as_ref()
        .map(|d| count_table(d, "runs") + count_table(d, "lines") + count_table(d, "last") + count_table(d, "exact"))
        .unwrap_or(0);
    if confirmed {
        if let Some(d) = &db {
            let _ = d.execute_batch("DELETE FROM runs; DELETE FROM lines; DELETE FROM last; DELETE FROM exact; VACUUM;");
        }
    }
    TargetReport { name: "learn".to_string(), rows, bytes, purged: confirmed }
}

fn purge_sessions_at(home: &Path, confirmed: bool) -> TargetReport {
    let files = session_files(&sessions_dir_path(home));
    let rows = files.len();
    let bytes = total_bytes(&files);
    if confirmed {
        for f in &files {
            let _ = std::fs::remove_file(f);
        }
    }
    TargetReport { name: "sessions".to_string(), rows, bytes, purged: confirmed }
}

fn purge_at(home: &Path, what: Purge, confirmed: bool) -> PurgeReport {
    let targets = match what {
        Purge::Index => vec![purge_index_at(home, confirmed)],
        Purge::Journal => vec![purge_journal_at(home, confirmed)],
        Purge::Learn => vec![purge_learn_at(home, confirmed)],
        Purge::Sessions => vec![purge_sessions_at(home, confirmed)],
        Purge::All => vec![
            purge_index_at(home, confirmed),
            purge_journal_at(home, confirmed),
            purge_learn_at(home, confirmed),
            purge_sessions_at(home, confirmed),
        ],
    };
    PurgeReport { confirmed, targets }
}

/// Deletes stored data for `what`. Without `confirmed` nothing is erased:
/// the report still says what would have been, in rows and bytes.
pub fn purge(what: Purge, confirmed: bool) -> PurgeReport {
    purge_at(&home(), what, confirmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_home() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir()
            .join(format!("bilro-ops-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn stats_em_ambiente_vazio_nao_quebra_e_devolve_zeros() {
        let home = temp_home();
        let s = stats_at(&home);
        assert_eq!(s.learned_commands, 0);
        assert_eq!(s.learned_mature, 0);
        assert_eq!(s.denoise_savings_bytes, 0);
        assert_eq!(s.indexed_chunks, 0);
        assert_eq!(s.journal_events_total, 0);
        assert!(s.journal_events_by_kind.is_empty());
        assert_eq!(s.sessions, 0);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn stats_conta_comandos_aprendidos_e_maduros() {
        let home = temp_home();
        let mut db = crate::learn::open(&learn_db_path(&home)).unwrap();
        for _ in 0..5 {
            crate::learn::observe(&mut db, "npm test", "linha 1\nlinha 2").unwrap();
        }
        crate::learn::observe(&mut db, "npm run build", "outra saida").unwrap();
        drop(db);

        let s = stats_at(&home);
        assert_eq!(s.learned_commands, 2, "esperava 2 assinaturas distintas");
        assert_eq!(s.learned_mature, 1, "so npm test passou de 3 execucoes");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn check_hooks_ignora_hook_de_outra_ferramenta_e_reprova_so_o_do_bilro() {
        let home = temp_home();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(
            settings_path(&home),
            serde_json::json!({
                "hooks": {
                    "PreToolUse": [
                        { "hooks": [{ "type": "command", "command": "node \"/nao/existe/outra-ferramenta.mjs\"" }] },
                        { "hooks": [{ "type": "command", "command": "/nao/existe/bilro hook bash" }] }
                    ]
                }
            })
            .to_string(),
        )
        .unwrap();

        let c = check_hooks(&home);
        assert!(!c.ok, "deveria reprovar por causa do bilro ausente, nao por causa da outra ferramenta");
        assert!(c.detalhe.contains("bilro"), "detalhe: {}", c.detalhe);
        assert!(!c.detalhe.contains("outra-ferramenta"), "vazou hook de outra ferramenta: {}", c.detalhe);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn doctor_devolve_um_check_por_item_e_falha_tem_detalhe() {
        let home = temp_home();
        let checks = doctor_at(&home);
        assert_eq!(checks.len(), 10);

        let falhando: Vec<&Check> = checks.iter().filter(|c| !c.ok).collect();
        assert!(!falhando.is_empty(), "home vazio deveria reprovar pelo menos um check");
        for c in &falhando {
            assert!(!c.detalhe.is_empty(), "check {} falhou sem dizer como consertar", c.nome);
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn purge_sem_confirmed_nao_apaga_mas_relata() {
        let home = temp_home();
        let mut db = crate::learn::open(&learn_db_path(&home)).unwrap();
        crate::learn::observe(&mut db, "cmd", "linha").unwrap();
        drop(db);

        let r = purge_at(&home, Purge::Learn, false);
        assert!(!r.confirmed);
        assert!(!r.targets[0].purged);
        assert!(r.targets[0].rows > 0, "deveria reportar o que apagaria");

        let db2 = crate::learn::open(&learn_db_path(&home)).unwrap();
        assert_eq!(count_table(&db2, "runs"), 1, "purge sem confirmed apagou mesmo assim");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn purge_com_confirmed_apaga_so_o_alvo_pedido() {
        let home = temp_home();
        let mut learn_db = crate::learn::open(&learn_db_path(&home)).unwrap();
        crate::learn::observe(&mut learn_db, "cmd", "linha").unwrap();
        drop(learn_db);
        let idx = crate::sandbox::open(&index_db_path(&home)).unwrap();
        crate::sandbox::index(&idx, "l", "corpo indexado", "fonte").unwrap();
        drop(idx);

        let r = purge_at(&home, Purge::Learn, true);
        assert!(r.targets[0].purged);

        let learn_db2 = crate::learn::open(&learn_db_path(&home)).unwrap();
        assert_eq!(count_table(&learn_db2, "runs"), 0, "learn nao foi limpo");
        drop(learn_db2);

        let idx2 = crate::sandbox::open(&index_db_path(&home)).unwrap();
        assert_eq!(count_table(&idx2, "chunks"), 1, "index foi apagado sem ter sido pedido");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn purge_all_zera_tudo_e_stats_depois_nao_quebra() {
        let home = temp_home();
        let mut learn_db = crate::learn::open(&learn_db_path(&home)).unwrap();
        crate::learn::observe(&mut learn_db, "cmd", "linha").unwrap();
        drop(learn_db);
        let idx = crate::sandbox::open(&index_db_path(&home)).unwrap();
        crate::sandbox::index(&idx, "l", "corpo", "fonte").unwrap();
        drop(idx);
        let jr = crate::journal::open(&journal_db_path(&home)).unwrap();
        crate::journal::record(&jr, "prompt", "assunto", "corpo", "s1", "proj").unwrap();
        drop(jr);
        std::fs::create_dir_all(sessions_dir_path(&home)).unwrap();
        std::fs::write(sessions_dir_path(&home).join("s1.json"), "{}").unwrap();

        let r = purge_at(&home, Purge::All, true);
        assert_eq!(r.targets.len(), 4);
        assert!(r.targets.iter().all(|t| t.purged));

        let s = stats_at(&home);
        assert_eq!(s.learned_commands, 0);
        assert_eq!(s.indexed_chunks, 0);
        assert_eq!(s.journal_events_total, 0);
        assert_eq!(s.sessions, 0);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    #[ignore = "diagnostico manual: roda contra a maquina real, nao contra um home isolado"]
    fn diagnostico_real_da_maquina() {
        println!("{:#?}", stats());
        for c in doctor() {
            println!("[{}] {} - {}", if c.ok { "ok" } else { "FALHA" }, c.nome, c.detalhe);
        }
    }
}
