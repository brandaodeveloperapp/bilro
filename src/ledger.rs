use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn home_dir() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn dir() -> PathBuf {
    home_dir().join(".claude").join("bilro").join("sessions")
}

fn file(session_id: &str) -> PathBuf {
    let id = if session_id.is_empty() { "unknown" } else { session_id };
    dir().join(format!("{id}.json"))
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn default_state() -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("dispatches".to_string(), json!([]));
    m.insert("withheld".to_string(), json!([]));
    m.insert("filtered".to_string(), json!([]));
    m
}

/// Loads a session's state, defaulting missing fields so callers never see
/// `null` where an array was expected.
pub fn read(session_id: &str) -> Value {
    let mut base = default_state();
    if let Ok(raw) = fs::read_to_string(file(session_id)) {
        if let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(&raw) {
            for (k, v) in parsed {
                base.insert(k, v);
            }
        }
    }
    Value::Object(base)
}

/// Writes a session's state atomically: a temp file per process id, then a
/// rename, so parallel dispatch never sees a half-written file.
pub fn write(session_id: &str, state: &Value) {
    let target = file(session_id);
    let Some(parent) = target.parent() else { return };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let tmp = PathBuf::from(format!("{}.{}.tmp", target.display(), std::process::id()));
    if fs::write(&tmp, state.to_string()).is_err() {
        return;
    }
    let _ = fs::rename(&tmp, &target);
}

/// Holds an exclusive lock for one session while its file is read and rewritten.
/// The atomic rename alone prevents a torn file but not a lost update: two
/// processes that read the same state both write their own version and one of
/// the two entries disappears.
struct Guard(PathBuf);

impl Guard {
    fn acquire(session_id: &str) -> Option<Guard> {
        let path = PathBuf::from(format!("{}.lock", file(session_id).display()));
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Some(Guard(path)),
                Err(_) => {
                    if stale(&path) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if std::time::Instant::now() >= deadline {
                        return None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| t.elapsed().map(|e| e.as_secs() > 30).unwrap_or(false))
        .unwrap_or(false)
}

/// Appends one entry to a named list (`dispatches`, `withheld`, `filtered`,
/// ...) inside a session and persists the result. The read and the write happen
/// under one lock so a concurrent append is never overwritten. If the lock
/// cannot be taken the entry is dropped rather than written unlocked: losing one
/// accounting row is bounded, overwriting another process's rows is not.
pub fn record(session_id: &str, kind: &str, mut entry: Value) -> Value {
    let Some(_guard) = Guard::acquire(session_id) else {
        return read(session_id);
    };
    let mut state = read(session_id);
    if let Value::Object(entry_map) = &mut entry {
        entry_map.insert("at".to_string(), json!(now_ms()));
    }
    if let Value::Object(state_map) = &mut state {
        let list = state_map.entry(kind.to_string()).or_insert_with(|| json!([]));
        if let Value::Array(arr) = list {
            arr.push(entry);
        }
    }
    write(session_id, &state);
    state
}

/// One session file on disk: its id and last-modified time in epoch ms.
pub struct SessionInfo {
    pub id: String,
    pub mtime: i64,
}

/// Lists known sessions, most recently modified first.
pub fn sessions() -> Vec<SessionInfo> {
    let Ok(entries) = fs::read_dir(dir()) else { return Vec::new() };
    let mut out: Vec<SessionInfo> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".json")?.to_string();
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            Some(SessionInfo { id, mtime })
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.mtime));
    out
}

/// What a session spent on fixed costs, and what it avoided spending.
pub struct Summary {
    pub dispatches: usize,
    pub dispatch_cost: f64,
    pub repeats: usize,
    pub withheld: f64,
    pub filtered: f64,
    pub saved: f64,
}

fn sum_field(list: &[Value], field: &str) -> f64 {
    list.iter().filter_map(|v| v.get(field)).filter_map(Value::as_f64).sum()
}

fn as_array<'a>(state: &'a Value, key: &str) -> &'a [Value] {
    state.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

pub fn summarize(session_id: &str) -> Summary {
    let state = read(session_id);
    let dispatches = as_array(&state, "dispatches");
    let withheld = as_array(&state, "withheld");
    let filtered = as_array(&state, "filtered");

    let dispatch_cost = sum_field(dispatches, "cost");
    let withheld_sum = sum_field(withheld, "tokens");
    let filtered_sum = sum_field(filtered, "saved");
    let repeats = dispatches.iter().filter(|d| d.get("repeat").and_then(Value::as_bool).unwrap_or(false)).count();

    Summary {
        dispatches: dispatches.len(),
        dispatch_cost,
        repeats,
        withheld: withheld_sum,
        filtered: filtered_sum,
        saved: withheld_sum + filtered_sum,
    }
}

/// Deletes session files older than `older_than_days`, returning how many
/// were removed.
pub fn prune(older_than_days: f64) -> usize {
    let cutoff = now_ms() - (older_than_days * DAY_MS as f64) as i64;
    let mut removed = 0usize;
    for s in sessions() {
        if s.mtime < cutoff && fs::remove_file(file(&s.id)).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_id() -> String {
        static N: AtomicU64 = AtomicU64::new(0);
        format!("test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst))
    }

    #[test]
    fn sessao_nova_comeca_vazia_e_nao_quebra() {
        let s = read(&unique_id());
        assert_eq!(s.get("dispatches").unwrap(), &json!([]));
        assert_eq!(s.get("withheld").unwrap(), &json!([]));
    }

    #[test]
    fn soma_custo_e_conta_repeticao() {
        let id = unique_id();
        write(
            &id,
            &json!({
                "dispatches": [
                    {"cost": 20000, "repeat": false},
                    {"cost": 21000, "repeat": true},
                ],
                "withheld": [{"tokens": 5000}],
                "filtered": [{"saved": 1000}],
            }),
        );
        let t = summarize(&id);
        assert_eq!(t.dispatches, 2);
        assert!((t.dispatch_cost - 41000.0).abs() < f64::EPSILON);
        assert_eq!(t.repeats, 1);
        assert!((t.saved - 6000.0).abs() < f64::EPSILON);
        let _ = fs::remove_file(file(&id));
    }

    #[test]
    fn record_acrescenta_sem_perder_o_que_ja_existia() {
        let id = unique_id();
        record(&id, "dispatches", json!({"cost": 1}));
        let s = record(&id, "dispatches", json!({"cost": 2}));
        assert_eq!(s.get("dispatches").unwrap().as_array().unwrap().len(), 2);
        let _ = fs::remove_file(file(&id));
    }
}

#[cfg(test)]
mod concurrency_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn append_concorrente_nao_perde_atualizacao() {
        let sess = format!("conc-{}-{}", std::process::id(), now_ms());
        let n = 10;
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let s = sess.clone();
                std::thread::spawn(move || {
                    record(&s, "dispatches", json!({ "id": i }));
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let state = read(&sess);
        let got = state["dispatches"].as_array().map(|a| a.len()).unwrap_or(0);
        let _ = fs::remove_file(file(&sess));
        assert_eq!(got, n, "perdeu {} de {} atualizacoes", n - got, n);
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lock_preso_faz_record_desistir_em_vez_de_escrever_sem_lock() {
        let sess = format!("held-{}-{}", std::process::id(), now_ms());
        record(&sess, "dispatches", json!({ "id": 0 }));
        let lock = PathBuf::from(format!("{}.lock", file(&sess).display()));
        fs::write(&lock, "").unwrap();
        let antes = read(&sess)["dispatches"].as_array().map(|a| a.len()).unwrap_or(0);
        let depois_state = record(&sess, "dispatches", json!({ "id": 1 }));
        let depois = depois_state["dispatches"].as_array().map(|a| a.len()).unwrap_or(0);
        let _ = fs::remove_file(&lock);
        let _ = fs::remove_file(file(&sess));
        assert_eq!(antes, depois, "nao pode escrever sem segurar o lock");
    }
}
