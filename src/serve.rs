use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

const PAGE: &str = include_str!("panel.html");

const ICONE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><circle cx="16" cy="9" r="4" fill="#b98adf"/><circle cx="9" cy="23" r="3" fill="#b98adf"/><circle cx="23" cy="23" r="3" fill="#b98adf"/><path d="M16 9L9 23M16 9l7 14M9 23h14" stroke="#6a4d8c" stroke-width="1.5" fill="none"/></svg>"##;

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
    let mut aprendidos = 0i64;
    let mut com_historico = 0i64;
    let mut bytes_antes = 0i64;
    let mut bytes_depois = 0i64;
    let mut barulhentos: Vec<serde_json::Value> = Vec::new();

    if let Ok(db) = crate::learn::open(&bilro_dir().join("learn.db")) {
        if let Ok(mut st) = db.prepare("SELECT r.sig, r.n, l.body FROM runs r LEFT JOIN last l ON l.sig = r.sig") {
            let rows = st.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?))
            });
            if let Ok(rows) = rows {
                for row in rows.flatten() {
                    let (sig, n, body) = row;
                    aprendidos += 1;
                    if n >= 3 {
                        com_historico += 1;
                    }
                    let Some(body) = body else { continue };
                    if body.is_empty() {
                        continue;
                    }
                    let antes = body.chars().count() as i64;
                    let depois = crate::learn::denoise(&db, &sig, &body, 3, 0.8)
                        .map(|d| d.text.chars().count() as i64)
                        .unwrap_or(antes);
                    bytes_antes += antes;
                    bytes_depois += depois;
                    if antes - depois > 0 {
                        barulhentos.push(serde_json::json!({
                            "comando": sig.chars().take(70).collect::<String>(),
                            "execucoes": n,
                            "cortado": antes - depois
                        }));
                    }
                }
            }
        }
    }
    barulhentos.sort_by_key(|b| -(b["cortado"].as_i64().unwrap_or(0)));
    barulhentos.truncate(8);

    let cwd = std::env::current_dir().unwrap_or_default();
    let projeto = crate::journal::project_of(&cwd);
    let eventos: Vec<serde_json::Value> = crate::journal::open_default()
        .ok()
        .and_then(|db| crate::journal::timeline(&db, Some(&projeto), 25).ok())
        .unwrap_or_default()
        .iter()
        .map(|e| {
            serde_json::json!({
                "tipo": e.kind,
                "assunto": e.subject,
                "corpo": e.body.lines().take(2).collect::<Vec<_>>().join(" "),
                "quando": e.at
            })
        })
        .collect();

    let sessoes = crate::ledger::sessions();
    let despachos: usize = sessoes.iter().map(|s| crate::ledger::summarize(&s.id).dispatches).sum();

    serde_json::json!({
        "agora": crate::ledger::now_ms(),
        "projeto": projeto,
        "aprendidos": aprendidos,
        "com_historico": com_historico,
        "cobertura": if aprendidos > 0 { com_historico as f64 / aprendidos as f64 } else { 0.0 },
        "bytes_antes": bytes_antes,
        "bytes_depois": bytes_depois,
        "economia": if bytes_antes > 0 { 1.0 - bytes_depois as f64 / bytes_antes as f64 } else { 0.0 },
        "barulhentos": barulhentos,
        "eventos": eventos,
        "sessoes": sessoes.len(),
        "despachos": despachos,
        "discos": {
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
fn grafo() -> serde_json::Value {
    let cwd = std::env::current_dir().unwrap_or_default();
    let dir = home()
        .join(".claude")
        .join("projects")
        .join(crate::journal::project_of(&cwd))
        .join("memory");
    let g = crate::graph::build(&dir);

    let entrando = |nome: &str| g.back.get(nome).map(|v| v.len()).unwrap_or(0);
    let saindo = |nome: &str| g.out.get(nome).map(|v| v.len()).unwrap_or(0);

    let nos: Vec<serde_json::Value> = g
        .memories
        .iter()
        .map(|m| {
            let dentro = entrando(&m.name);
            let fora = saindo(&m.name);
            let primeira = m
                .body
                .lines()
                .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .unwrap_or("")
                .chars()
                .take(160)
                .collect::<String>();
            serde_json::json!({
                "id": m.name,
                "tipo": m.kind.clone().unwrap_or_else(|| "sem tipo".into()),
                "entrando": dentro,
                "saindo": fora,
                "orfa": dentro == 0 && fora == 0,
                "resumo": crate::redact::redact(&primeira),
                "verificavel": m.verify.is_some(),
            })
        })
        .collect();

    let mut arestas = Vec::new();
    for (de, alvos) in &g.out {
        for para in alvos {
            arestas.push(serde_json::json!({
                "de": de,
                "para": para,
                "quebrada": !g.names.contains(para),
            }));
        }
    }

    let fantasmas: Vec<serde_json::Value> = arestas
        .iter()
        .filter(|a| a["quebrada"] == true)
        .filter_map(|a| a["para"].as_str().map(String::from))
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .map(|nome| {
            serde_json::json!({
                "id": nome, "tipo": "nao existe", "entrando": 0, "saindo": 0,
                "orfa": false, "resumo": "esta memoria e citada mas nunca foi escrita",
                "verificavel": false, "fantasma": true
            })
        })
        .collect();

    let todos: Vec<serde_json::Value> = nos.into_iter().chain(fantasmas).collect();
    let caminho = dir.display().to_string();
    serde_json::json!({ "nos": todos, "arestas": arestas, "dir": caminho })
}

fn respond(stream: &mut TcpStream, status: &str, tipo: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {tipo}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

fn handle(mut stream: TcpStream) {
    let mut linha = String::new();
    if BufReader::new(&stream).read_line(&mut linha).is_err() {
        return;
    }
    let rota = linha.split_whitespace().nth(1).unwrap_or("/");
    match rota {
        "/api/grafo" => respond(&mut stream, "200 OK", "application/json; charset=utf-8", &grafo().to_string()),
        "/api/estado" => respond(&mut stream, "200 OK", "application/json; charset=utf-8", &snapshot().to_string()),
        "/favicon.ico" => respond(&mut stream, "200 OK", "image/svg+xml", ICONE),
        "/" | "/index.html" => respond(&mut stream, "200 OK", "text/html; charset=utf-8", PAGE),
        _ => respond(&mut stream, "404 Not Found", "text/plain; charset=utf-8", "nao existe"),
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
        Err(e) => return eprintln!("  nao consegui abrir a porta {port}: {e}"),
    };
    let addr = listener.local_addr().map(|a| a.to_string()).unwrap_or_default();
    println!("\n  painel do bilro em http://{addr}\n  ctrl-c para parar\n");
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || handle(stream));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estado_e_json_com_os_campos_que_a_pagina_espera() {
        let s = snapshot();
        for campo in ["aprendidos", "cobertura", "economia", "eventos", "barulhentos", "discos", "projeto"] {
            assert!(s.get(campo).is_some(), "faltou {campo}");
        }
        assert!(s["cobertura"].as_f64().unwrap() >= 0.0);
        assert!(s["economia"].as_f64().unwrap() <= 1.0);
    }

    #[test]
    fn favicon_e_servido_para_nao_sujar_o_console() {
        assert!(ICONE.contains("<svg"), "icone invalido");
    }

    #[test]
    fn grafo_tem_nos_arestas_e_marca_link_quebrado() {
        let g = grafo();
        assert!(g["nos"].is_array(), "faltou nos");
        assert!(g["arestas"].is_array(), "faltou arestas");
        for a in g["arestas"].as_array().unwrap() {
            assert!(a["quebrada"].is_boolean(), "aresta sem marca de quebrada");
        }
        for n in g["nos"].as_array().unwrap() {
            assert!(n["id"].is_string() && !n["id"].as_str().unwrap().is_empty());
            assert!(n["entrando"].is_number() && n["saindo"].is_number());
        }
    }

    #[test]
    fn memoria_citada_e_nunca_escrita_vira_no_fantasma() {
        let g = grafo();
        let nos = g["nos"].as_array().unwrap();
        let quebradas: Vec<&str> = g["arestas"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|a| a["quebrada"] == true)
            .map(|a| a["para"].as_str().unwrap())
            .collect();
        for alvo in quebradas {
            assert!(
                nos.iter().any(|n| n["id"] == alvo),
                "aresta quebrada aponta para {alvo}, que nao virou no"
            );
        }
    }

    #[test]
    fn pagina_embutida_no_binario_nao_esta_vazia() {
        assert!(PAGE.contains("<html") || PAGE.contains("<!DOCTYPE"), "pagina invalida");
        assert!(PAGE.contains("/api/estado"), "a pagina precisa buscar o estado");
    }

    #[test]
    fn so_escuta_no_loopback() {
        let l = bind(0).expect("deveria conseguir abrir uma porta livre");
        let addr = l.local_addr().unwrap();
        assert!(addr.ip().is_loopback(), "escutando em {addr}, que nao e loopback");
        assert!(!addr.ip().is_unspecified(), "escutando em todas as interfaces");
    }

    #[test]
    fn rota_desconhecida_nao_serve_a_pagina() {
        let l = bind(0).unwrap();
        let porta = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in l.incoming().flatten() {
                handle(stream);
            }
        });
        let pedir = |rota: &str| -> String {
            use std::io::Read;
            let mut c = std::net::TcpStream::connect(("127.0.0.1", porta)).unwrap();
            let _ = c.write_all(format!("GET {rota} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes());
            let mut buf = String::new();
            let _ = c.read_to_string(&mut buf);
            buf
        };
        assert!(pedir("/").contains("200 OK"), "a raiz deveria servir a pagina");
        assert!(pedir("/api/estado").contains("application/json"), "a api deveria devolver json");
        let outra = pedir("/../../etc/passwd");
        assert!(outra.contains("404"), "rota desconhecida deveria dar 404");
        assert!(!outra.contains("<html"), "nunca servir a pagina para rota desconhecida");
    }
}
