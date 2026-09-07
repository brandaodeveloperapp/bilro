use crate::learn::is_severe;
use crate::shapes::contract::{Compressed, Shape};
use once_cell::sync::Lazy;
use regex::{Regex, RegexBuilder};
use serde_json::Value;

const MASK: &str = "***MASCARADO***";
const STRING_TRUNC: usize = 120;
const ARRAY_COLLAPSE_THRESHOLD: usize = 3;

static KV_LINE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\s*)([A-Za-z_][A-Za-z0-9_.-]*)(\s*[:=]\s*)(.*)$").unwrap());

static SECRET_KEY_RE: Lazy<Regex> = Lazy::new(|| {
    RegexBuilder::new(
        r"token|secret|password|passwd|senha|credential|authorization|bearer|api[_-]?key|access[_-]?key|private[_-]?key|client[_-]?secret|^KEY$|_KEY$",
    )
    .case_insensitive(true)
    .build()
    .unwrap()
});

static CREDENTIAL_IN_URL: Lazy<Regex> = Lazy::new(|| {
    RegexBuilder::new(r"([a-z][a-z0-9+.-]*://)([^\s:@/]*):([^\s@/]+)@").case_insensitive(true).build().unwrap()
});

#[derive(Default)]
struct Stats {
    masked: usize,
    collapsed: usize,
    truncated: usize,
}

/// Masks a password embedded in a connection string, even when the key name gives no hint.
fn mask_embedded(value: &str) -> String {
    CREDENTIAL_IN_URL.replace_all(value, |caps: &regex::Captures| format!("{}{}:{}@", &caps[1], &caps[2], MASK)).into_owned()
}

fn parse_whole_json(lines: &[&str]) -> Option<Value> {
    let text = lines.join("\n");
    let trimmed = text.trim();
    let first = trimmed.chars().next()?;
    if first != '{' && first != '[' {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn shape_signature(item: &Value) -> String {
    match item {
        Value::Null => "null".to_string(),
        Value::Array(_) => "array".to_string(),
        Value::Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            keys.join(",")
        }
        Value::String(_) => "string".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::Bool(_) => "boolean".to_string(),
    }
}

fn truncate_string(value: &str, stats: &mut Stats) -> String {
    let char_count = value.chars().count();
    if char_count <= STRING_TRUNC || is_severe(value) {
        return value.to_string();
    }
    stats.truncated += 1;
    let head: String = value.chars().take(STRING_TRUNC).collect();
    format!("{}...(cortado, {} chars originais)", head, char_count)
}

fn shrink_value(value: &Value, stats: &mut Stats) -> Value {
    match value {
        Value::Array(arr) => {
            if arr.len() > ARRAY_COLLAPSE_THRESHOLD {
                let sig = shape_signature(&arr[0]);
                let homogeneous = arr.iter().all(|v| shape_signature(v) == sig);
                if homogeneous {
                    stats.collapsed += arr.len() - 1;
                    let mut out = serde_json::Map::new();
                    out.insert(
                        "(resumo)".to_string(),
                        Value::String(format!("{} itens no mesmo formato, primeiro como esquema", arr.len())),
                    );
                    out.insert("(exemplo)".to_string(), shrink_value(&arr[0], stats));
                    return Value::Object(out);
                }
            }
            Value::Array(arr.iter().map(|v| shrink_value(v, stats)).collect())
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, v) in map.iter() {
                if SECRET_KEY_RE.is_match(key) {
                    out.insert(key.clone(), Value::String(MASK.to_string()));
                    stats.masked += 1;
                } else {
                    let shrunk = shrink_value(v, stats);
                    if let Value::String(s) = &shrunk {
                        let safe = mask_embedded(s);
                        if safe != *s {
                            stats.masked += 1;
                        }
                        out.insert(key.clone(), Value::String(safe));
                    } else {
                        out.insert(key.clone(), shrunk);
                    }
                }
            }
            Value::Object(out)
        }
        Value::String(s) => Value::String(truncate_string(s, stats)),
        other => other.clone(),
    }
}

fn compress_json(parsed: &Value) -> (String, String) {
    let mut stats = Stats::default();
    let shrunk = shrink_value(parsed, &mut stats);
    let text = serde_json::to_string_pretty(&shrunk).unwrap();
    let note = format!(
        "keyvalue(json): {} valor(es) mascarado(s), {} item(ns) de array colapsado(s), {} string(s) truncada(s)",
        stats.masked, stats.collapsed, stats.truncated
    );
    (text, note)
}

fn compress_kv(lines: &[&str]) -> Compressed {
    let mut masked = 0usize;
    let mut truncated = 0usize;
    let out: Vec<String> = lines
        .iter()
        .map(|&line| {
            let Some(caps) = KV_LINE_RE.captures(line) else {
                return line.to_string();
            };
            let indent = &caps[1];
            let key = &caps[2];
            let sep = &caps[3];
            let raw_value = &caps[4];
            let value = if SECRET_KEY_RE.is_match(key) {
                masked += 1;
                MASK.to_string()
            } else {
                let safe = mask_embedded(raw_value);
                if safe != raw_value {
                    masked += 1;
                    safe
                } else if raw_value.chars().count() > STRING_TRUNC && !is_severe(line) {
                    truncated += 1;
                    let head: String = raw_value.chars().take(STRING_TRUNC).collect();
                    format!("{}...(cortado, {} chars originais)", head, raw_value.chars().count())
                } else {
                    raw_value.to_string()
                }
            };
            format!("{}{}{}{}", indent, key, sep, value)
        })
        .collect();
    let text = out.join("\n");
    let note = format!("keyvalue: {} valor(es) mascarado(s), {} truncado(s)", masked, truncated);
    Compressed { text, dropped: 0, note }
}

/// Stable identifier for this shape.
pub fn name() -> &'static str {
    "keyvalue"
}

/// Confidence, 0..1, that these lines are JSON or `KEY=value` / `key: value` pairs.
pub fn detect(lines: &[&str]) -> f64 {
    if parse_whole_json(lines).is_some() {
        return 1.0;
    }
    let non_empty: Vec<&str> = lines.iter().copied().filter(|l| !l.trim().is_empty()).collect();
    if non_empty.len() < 2 {
        return 0.0;
    }
    let hits = non_empty.iter().filter(|l| KV_LINE_RE.is_match(l)).count();
    hits as f64 / non_empty.len() as f64
}

/// Masks secrets by key name and by connection-string shape, collapses homogeneous
/// arrays, and truncates long strings, never touching a line `learn::is_severe` flags.
pub fn compress(lines: &[&str]) -> Compressed {
    if let Some(parsed) = parse_whole_json(lines) {
        let (text, note) = compress_json(&parsed);
        let dropped = lines.len().saturating_sub(text.split('\n').count());
        return Compressed { text, dropped, note };
    }
    compress_kv(lines)
}

/// Assembles the shape descriptor for the keyvalue dispatcher.
pub fn shape() -> Shape {
    Shape { name: name(), detect, compress }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::install_log::detect as detect_install;
    use crate::shapes::listing::detect as detect_listing;

    fn load(fixture: &str) -> String {
        let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), fixture);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {} ausente: {}", fixture, e))
    }

    fn lines_of(text: &str) -> Vec<&str> {
        text.split('\n').collect()
    }

    #[test]
    fn nome_estavel() {
        assert_eq!(name(), "keyvalue");
    }

    #[test]
    fn detecta_json_real_curl_github_releases_com_confianca_maxima() {
        let text = load("keyvalue-curl-real.json");
        assert_eq!(detect(&lines_of(&text)), 1.0);
    }

    #[test]
    fn detecta_json_real_node_process_versions_com_confianca_maxima() {
        let text = load("keyvalue-versions-real.json");
        assert_eq!(detect(&lines_of(&text)), 1.0);
    }

    #[test]
    fn detecta_env_real_key_value_com_alta_confianca() {
        let text = load("keyvalue-env-real.txt");
        assert!(detect(&lines_of(&text)) >= 0.6);
    }

    #[test]
    fn nao_detecta_listagem_de_arquivo_como_keyvalue() {
        let text = load("listing-find.txt");
        assert!(detect(&lines_of(&text)) < 0.6);
        let grep_text = load("listing-grep.txt");
        assert!(detect(&lines_of(&grep_text)) < 0.6);
    }

    #[test]
    fn nao_detecta_install_log_como_keyvalue() {
        let text = load("install-npm-real.txt");
        assert!(detect(&lines_of(&text)) < 0.6);
        let pip_text = load("install-pip-real.txt");
        assert!(detect(&lines_of(&pip_text)) < 0.6);
    }

    #[test]
    fn nao_detecta_diff_de_git_como_keyvalue() {
        let text = load("git-diff-real.txt");
        assert!(detect(&lines_of(&text)) < 0.6);
    }

    #[test]
    fn listing_e_install_log_nao_se_confundem_com_keyvalue_no_sentido_inverso() {
        let text = load("keyvalue-versions-real.json");
        assert!(detect_listing(&lines_of(&text)) < 0.6);
        assert!(detect_install(&lines_of(&text)) < 0.6);
    }

    #[test]
    fn compress_de_json_real_colapsa_array_homogeneo_longo_e_trunca_string_longa_reduzindo_muito_os_bytes() {
        let text = load("keyvalue-curl-real.json");
        let before = text.len();
        let r = compress(&lines_of(&text));
        let cut = 1.0 - (r.text.len() as f64 / before as f64);
        assert!(cut > 0.9, "esperava corte > 90%, obteve {:.1}%", cut * 100.0);
        let parsed: Value = serde_json::from_str(&r.text).unwrap();
        assert!(parsed["(resumo)"].as_str().unwrap().contains("5 itens"));
        assert_eq!(parsed["(exemplo)"]["tag_name"], "v26.8.1");
        assert!(parsed["(exemplo)"]["body"].as_str().unwrap().contains("cortado"));
        assert!(r.note.contains("truncada"));
    }

    #[test]
    fn compress_de_json_simples_process_versions_preserva_todas_as_chaves_sem_colapso_de_array() {
        let text = load("keyvalue-versions-real.json");
        let parsed_before: Value = serde_json::from_str(&text).unwrap();
        let r = compress(&lines_of(&text));
        let parsed_after: Value = serde_json::from_str(&r.text).unwrap();
        let mut before_keys: Vec<&String> = parsed_before.as_object().unwrap().keys().collect();
        let mut after_keys: Vec<&String> = parsed_after.as_object().unwrap().keys().collect();
        before_keys.sort();
        after_keys.sort();
        assert_eq!(after_keys, before_keys);
        assert_eq!(parsed_after["node"], parsed_before["node"]);
    }

    #[test]
    fn compress_de_env_real_trunca_path_gigante_mas_preserva_todas_as_chaves() {
        let text = load("keyvalue-env-real.txt");
        let keys_before: Vec<&str> =
            lines_of(&text).into_iter().filter(|l| l.contains('=')).map(|l| l.split('=').next().unwrap()).collect();
        let r = compress(&lines_of(&text));
        for key in keys_before {
            assert!(r.text.contains(&format!("{}=", key)), "chave {} sumiu", key);
        }
        assert!(r.text.len() < text.len());
    }

    #[test]
    fn seguranca_chave_que_parece_segredo_tem_o_valor_mascarado_em_formato_key_value() {
        let synthetic = [
            "DATABASE_URL=postgres://user:pass@host:5432/db",
            "API_KEY=sk_live_abcdefghijklmnopqrstuvwxyz123456",
            "AUTH_TOKEN=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.fake.sig",
            "PASSWORD=hunter2superSecret",
            "SENHA=trocarDepois123",
            "AUTHORIZATION=Bearer abc.def.ghi",
            "HOME=/Users/igorbrandao",
        ]
        .join("\n");
        let r = compress(&lines_of(&synthetic));
        assert!(!r.text.contains("sk_live_abcdefghijklmnopqrstuvwxyz123456"));
        assert!(!r.text.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.fake.sig"));
        assert!(!r.text.contains("hunter2superSecret"));
        assert!(!r.text.contains("trocarDepois123"));
        assert!(!r.text.contains("Bearer abc.def.ghi"));
        assert!(r.text.contains("HOME=/Users/igorbrandao"), "chave nao-secreta deve sobreviver intacta");
        assert!(r.text.contains("API_KEY=***MASCARADO***"));
        assert!(r.note.contains("mascarado"));
    }

    #[test]
    fn seguranca_chave_que_parece_segredo_tem_o_valor_mascarado_em_json_aninhado() {
        let synthetic = serde_json::json!({
            "service": "billing",
            "config": {
                "apiKey": "sk_live_shouldnotleak",
                "nested": { "password": "hunter2", "bearerToken": "abc123xyz" }
            },
            "port": 8080
        })
        .to_string();
        let r = compress(&lines_of(&synthetic));
        assert!(!r.text.contains("sk_live_shouldnotleak"));
        assert!(!r.text.contains("hunter2"));
        assert!(!r.text.contains("abc123xyz"));
        let parsed: Value = serde_json::from_str(&r.text).unwrap();
        assert_eq!(parsed["config"]["apiKey"], "***MASCARADO***");
        assert_eq!(parsed["config"]["nested"]["password"], "***MASCARADO***");
        assert_eq!(parsed["config"]["nested"]["bearerToken"], "***MASCARADO***");
        assert_eq!(parsed["port"], 8080);
        assert_eq!(parsed["service"], "billing");
    }

    #[test]
    fn linha_severa_nao_tem_o_valor_truncado_so_mascarado_quando_aplicavel() {
        let big_error = "x".repeat(400);
        let line = format!("LAST_ERROR=failed to connect: {}", big_error);
        let r = compress(&lines_of(&line));
        assert!(r.text.contains(&big_error), "erro grande nao deveria ser truncado");
    }

    #[test]
    fn compress_retorna_string_valida_para_uma_unica_linha_key_value_simples() {
        let r = compress(&["FOO=bar"]);
        assert_eq!(r.text, "FOO=bar");
        assert_eq!(r.dropped, 0);
    }

    #[test]
    fn senha_embutida_em_string_de_conexao_nunca_chega_ao_contexto() {
        let linhas = vec![
            "DATABASE_URL=postgres://user:s3nh4Sup3rS3cr3t@db:5432/prod",
            "REDIS_URL=redis://:mypassword@cache:6379",
            "MONGO=mongodb://admin:Adm1nPass@mongo:27017/db",
            "AMQP=amqp://guest:guestpw@rabbit:5672",
        ];
        let out = compress(&linhas).text;
        for segredo in ["s3nh4Sup3rS3cr3t", "mypassword", "Adm1nPass", "guestpw"] {
            assert!(!out.contains(segredo), "vazou {}", segredo);
        }
        assert!(out.contains("db:5432/prod"));
        assert!(out.contains("cache:6379"));
    }

    #[test]
    fn chave_que_so_contem_a_palavra_key_por_acaso_nao_e_mascarada() {
        let out = compress(&["primaryKey=id", "monkeyName=george", "keyboard_layout=abnt2", "publicKeyPath=/etc/x.pub"]).text;
        assert!(out.contains("primaryKey=id"));
        assert!(out.contains("monkeyName=george"));
        assert!(out.contains("keyboard_layout=abnt2"));
        assert!(out.contains("publicKeyPath=/etc/x.pub"));
    }

    #[test]
    fn url_sem_credencial_passa_intacta() {
        let out = compress(&["OK=https://example.com/path", "GIT=git@github.com:user/repo.git"]).text;
        assert!(out.contains("https://example.com/path"));
        assert!(out.contains("git@github.com:user/repo.git"));
    }
}

