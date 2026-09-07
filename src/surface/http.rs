use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

const PAGE: &str = include_str!("panel.html");

const ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><circle cx="16" cy="9" r="4" fill="#b98adf"/><circle cx="9" cy="23" r="3" fill="#b98adf"/><circle cx="23" cy="23" r="3" fill="#b98adf"/><path d="M16 9L9 23M16 9l7 14M9 23h14" stroke="#6a4d8c" stroke-width="1.5" fill="none"/></svg>"##;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

fn bilro_dir() -> PathBuf {
    home().join(".claude").join("bilro")
}

fn file_size(name: &str) -> u64 {
    std::fs::metadata(bilro_dir().join(name)).map(|m| m.len()).unwrap_or(0)
}

/// Everything the panel shows, measured rather than estimated: the savings are
/// produced by replaying denoise over the output each command last printed.
fn snapshot() -> serde_json::Value {
    let mut learned = 0i64;
    let mut with_history = 0i64;
    let mut bytes_before = 0i64;
    let mut bytes_after = 0i64;
    let mut noisy: Vec<serde_json::Value> = Vec::new();

    if let Ok(db) = crate::compress::learn::open(&bilro_dir().join("learn.db")) {
        if let Ok(mut st) = db.prepare("SELECT r.sig, r.n, l.body FROM runs r LEFT JOIN last l ON l.sig = r.sig") {
            let rows = st.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?))
            });
            if let Ok(rows) = rows {
                for row in rows.flatten() {
                    let (sig, n, body) = row;
                    learned += 1;
                    if n >= 3 {
                        with_history += 1;
                    }
                    let Some(body) = body else { continue };
                    if body.is_empty() {
                        continue;
                    }
                    let before = body.chars().count() as i64;
                    let after = crate::compress::learn::denoise(&db, &sig, &body, 3, 0.8)
                        .map(|d| d.text.chars().count() as i64)
                        .unwrap_or(before);
                    bytes_before += before;
                    bytes_after += after;
                    if before - after > 0 {
                        noisy.push(serde_json::json!({
                            "command": sig.chars().take(70).collect::<String>(),
                            "runs": n,
                            "trimmed": before - after
                        }));
                    }
                }
            }
        }
    }
    noisy.sort_by_key(|b| -(b["trimmed"].as_i64().unwrap_or(0)));
    noisy.truncate(8);

    let cwd = std::env::current_dir().unwrap_or_default();
    let project = crate::store::journal::project_of(&cwd);
    let events: Vec<serde_json::Value> = crate::store::journal::open_default()
        .ok()
        .and_then(|db| crate::store::journal::timeline(&db, Some(&project), 25).ok())
        .unwrap_or_default()
        .iter()
        .map(|e| {
            serde_json::json!({
                "kind": e.kind,
                "subject": e.subject,
                "body": e.body.lines().take(2).collect::<Vec<_>>().join(" "),
                "at": e.at
            })
        })
        .collect();

    let sessions = crate::store::ledger::sessions();
    let dispatches: usize = sessions.iter().map(|s| crate::store::ledger::summarize(&s.id).dispatches).sum();

    serde_json::json!({
        "now": crate::store::ledger::now_ms(),
        "project": project,
        "learned": learned,
        "with_history": with_history,
        "coverage": if learned > 0 { with_history as f64 / learned as f64 } else { 0.0 },
        "bytes_before": bytes_before,
        "bytes_after": bytes_after,
        "savings": if bytes_before > 0 { 1.0 - bytes_after as f64 / bytes_before as f64 } else { 0.0 },
        "noisy": noisy,
        "events": events,
        "sessions": sessions.len(),
        "dispatches": dispatches,
        "disks": {
            "learn.db": file_size("learn.db"),
            "journal.db": file_size("journal.db"),
            "index.db": file_size("index.db"),
        }
    })
}

/// The memory network as nodes and edges. A note's weight is how often other
/// notes point at it, which is what makes a hub visible at a glance; an edge
/// pointing at a name nobody wrote is marked rather than dropped, because a
/// broken link is the interesting kind.
fn graph() -> serde_json::Value {
    let cwd = std::env::current_dir().unwrap_or_default();
    let dir = home()
        .join(".claude")
        .join("projects")
        .join(crate::store::journal::project_of(&cwd))
        .join("memory");
    let g = crate::store::graph::build(&dir);

    let incoming = |name: &str| g.back.get(name).map(|v| v.len()).unwrap_or(0);
    let outgoing = |name: &str| g.out.get(name).map(|v| v.len()).unwrap_or(0);

    let nodes: Vec<serde_json::Value> = g
        .memories
        .iter()
        .map(|m| {
            let inn = incoming(&m.name);
            let out = outgoing(&m.name);
            let first = m
                .body
                .lines()
                .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .unwrap_or("")
                .chars()
                .take(160)
                .collect::<String>();
            serde_json::json!({
                "id": m.name,
                "kind": m.kind.clone().unwrap_or_else(|| "no type".into()),
                "incoming": inn,
                "outgoing": out,
                "orphan": inn == 0 && out == 0,
                "summary": crate::redact::redact(&first),
                "verifiable": m.verify.is_some(),
            })
        })
        .collect();

    let mut edges = Vec::new();
    for (from, targets) in &g.out {
        for to in targets {
            edges.push(serde_json::json!({
                "from": from,
                "to": to,
                "broken": !g.names.contains(to),
            }));
        }
    }

    let ghosts: Vec<serde_json::Value> = edges
        .iter()
        .filter(|a| a["broken"] == true)
        .filter_map(|a| a["to"].as_str().map(String::from))
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .map(|name| {
            serde_json::json!({
                "id": name, "kind": "does not exist", "incoming": 0, "outgoing": 0,
                "orphan": false, "summary": "this memory is cited but was never written",
                "verifiable": false, "ghost": true
            })
        })
        .collect();

    let all: Vec<serde_json::Value> = nodes.into_iter().chain(ghosts).collect();
    let path = dir.display().to_string();
    serde_json::json!({ "nodes": all, "edges": edges, "dir": path })
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

fn handle(mut stream: TcpStream) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let route = line.split_whitespace().nth(1).unwrap_or("/");
    match route {
        "/api/graph" => respond(&mut stream, "200 OK", "application/json; charset=utf-8", &graph().to_string()),
        "/api/state" => respond(&mut stream, "200 OK", "application/json; charset=utf-8", &snapshot().to_string()),
        "/favicon.ico" => respond(&mut stream, "200 OK", "image/svg+xml", ICON),
        "/" | "/index.html" => respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE),
        _ => respond(&mut stream, "404 Not Found", "text/plain; charset=utf-8", "not found"),
    }
}

/// Binds the panel to the loopback interface only. It serves a journal of what
/// was asked and what failed; it has no business being reachable from another
/// machine.
pub fn bind(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
}

/// Serves the panel, one thread per connection.
pub fn serve(port: u16) {
    let listener = match bind(port) {
        Ok(l) => l,
        Err(e) => return eprintln!("  could not open port {port}: {e}"),
    };
    let addr = listener.local_addr().map(|a| a.to_string()).unwrap_or_default();
    println!("\n  bilro panel at http://{addr}\n  ctrl-c to stop\n");
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || handle(stream));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_is_json_with_the_fields_the_page_expects() {
        let s = snapshot();
        for field in ["learned", "coverage", "savings", "events", "noisy", "disks", "project"] {
            assert!(s.get(field).is_some(), "missing {field}");
        }
        assert!(s["coverage"].as_f64().unwrap() >= 0.0);
        assert!(s["savings"].as_f64().unwrap() <= 1.0);
    }

    #[test]
    fn favicon_is_served_to_keep_the_console_quiet() {
        assert!(ICON.contains("<svg"), "invalid icon");
    }

    #[test]
    fn graph_has_nodes_edges_and_marks_broken_links() {
        let g = graph();
        assert!(g["nodes"].is_array(), "missing nodes");
        assert!(g["edges"].is_array(), "missing edges");
        for a in g["edges"].as_array().unwrap() {
            assert!(a["broken"].is_boolean(), "edge without a broken flag");
        }
        for n in g["nodes"].as_array().unwrap() {
            assert!(n["id"].is_string() && !n["id"].as_str().unwrap().is_empty());
            assert!(n["incoming"].is_number() && n["outgoing"].is_number());
        }
    }

    #[test]
    fn memory_cited_but_never_written_becomes_a_ghost_node() {
        let g = graph();
        let nodes = g["nodes"].as_array().unwrap();
        let broken: Vec<&str> = g["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["broken"] == true)
            .map(|a| a["to"].as_str().unwrap())
            .collect();
        for target in broken {
            assert!(
                nodes.iter().any(|n| n["id"] == target),
                "broken edge points to {target}, which did not become a node"
            );
        }
    }

    #[test]
    fn page_embedded_in_the_binary_is_not_empty() {
        assert!(PAGE.contains("<html") || PAGE.contains("<!DOCTYPE"), "invalid page");
        assert!(PAGE.contains("/api/state"), "the page needs to fetch the state");
    }

    #[test]
    fn only_listens_on_loopback() {
        let l = bind(0).expect("should be able to open a free port");
        let addr = l.local_addr().unwrap();
        assert!(addr.ip().is_loopback(), "listening on {addr}, which is not loopback");
        assert!(!addr.ip().is_unspecified(), "listening on every interface");
    }

    #[test]
    fn unknown_route_does_not_serve_the_page() {
        let l = bind(0).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in l.incoming().flatten() {
                handle(stream);
            }
        });
        let request = |route: &str| -> String {
            use std::io::Read;
            let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            let _ = c.write_all(format!("GET {route} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes());
            let mut buf = String::new();
            let _ = c.read_to_string(&mut buf);
            buf
        };
        assert!(request("/").contains("200 OK"), "the root should serve the page");
        assert!(request("/api/state").contains("application/json"), "the api should return json");
        let other = request("/../../etc/passwd");
        assert!(other.contains("404"), "an unknown route should give a 404");
        assert!(!other.contains("<html"), "never serve the page for an unknown route");
    }
}
