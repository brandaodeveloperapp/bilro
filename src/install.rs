use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const HOOKS: &[(&str, Option<&str>, &str)] = &[
    ("PreToolUse", Some("Bash"), "hook bash"),
    ("PreToolUse", Some("Task"), "hook task"),
    ("PostToolUse", Some("Bash"), "hook shadow"),
    ("SessionStart", None, "hook session"),
];

pub struct Report {
    pub binary: PathBuf,
    pub linked: Option<PathBuf>,
    pub added: Vec<String>,
    pub already: Vec<String>,
    pub replaced: Vec<String>,
    pub backup: Option<PathBuf>,
}

fn settings_path(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

/// Puts the binary on the path and registers the hooks, keeping a copy of the
/// settings file it changes. Registering the same hook twice would double every
/// message, so an entry already pointing at this binary is left alone.
pub fn install(home: &Path, binary: &Path, link_dir: Option<&Path>) -> std::io::Result<Report> {
    let mut report = Report {
        binary: binary.to_path_buf(),
        linked: None,
        added: Vec::new(),
        already: Vec::new(),
        replaced: Vec::new(),
        backup: None,
    };

    if let Some(dir) = link_dir {
        std::fs::create_dir_all(dir)?;
        let link = dir.join("bilro");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(binary, &link)?;
        report.linked = Some(link);
    }

    let path = settings_path(home);
    let mut settings: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));

    if path.exists() {
        let backup = path.with_extension(format!("json.bak-{}", crate::ledger::now_ms()));
        std::fs::copy(&path, &backup)?;
        report.backup = Some(backup);
    }

    let me = binary.display().to_string();
    let hooks = settings.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!({}));

    for (event, matcher, sub) in HOOKS {
        let command = format!("{me} {sub}");
        let groups = hooks
            .as_object_mut()
            .unwrap()
            .entry(event.to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap();

        let ja_registrado = groups.iter().any(|g| {
            g.get("hooks")
                .and_then(|h| h.as_array())
                .map(|arr| arr.iter().any(|h| h.get("command").and_then(|c| c.as_str()) == Some(command.as_str())))
                .unwrap_or(false)
        });
        if ja_registrado {
            report.already.push(format!("{event}/{}", matcher.unwrap_or("*")));
            continue;
        }

        let group = groups.iter_mut().find(|g| {
            g.get("matcher").and_then(|m| m.as_str()) == *matcher
                || (matcher.is_none() && g.get("matcher").is_none())
        });

        match group {
            Some(g) => {
                let list = g.as_object_mut().unwrap().entry("hooks").or_insert_with(|| json!([]));
                let arr = list.as_array_mut().unwrap();
                let stale = arr.iter().position(|h| {
                    h.get("command").and_then(|c| c.as_str()).map(|c| c.ends_with(sub)).unwrap_or(false)
                });
                match stale {
                    Some(i) => {
                        arr[i] = json!({ "type": "command", "command": command });
                        report.replaced.push(format!("{event}/{}", matcher.unwrap_or("*")));
                    }
                    None => {
                        arr.push(json!({ "type": "command", "command": command }));
                        report.added.push(format!("{event}/{}", matcher.unwrap_or("*")));
                    }
                }
            }
            None => {
                let mut entry = serde_json::Map::new();
                if let Some(m) = matcher {
                    entry.insert("matcher".into(), json!(m));
                }
                entry.insert("hooks".into(), json!([{ "type": "command", "command": command }]));
                groups.push(Value::Object(entry));
                report.added.push(format!("{event}/{}", matcher.unwrap_or("*")));
            }
        }
    }

    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, serde_json::to_string_pretty(&settings)?)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!(
            "bilro-inst-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".claude")).unwrap();
        d
    }

    #[test]
    fn registra_os_quatro_hooks_num_ambiente_limpo() {
        let home = temp();
        let r = install(&home, Path::new("/opt/bilro"), None).unwrap();
        assert_eq!(r.added.len(), 4, "adicionados: {:?}", r.added);
        let s: Value = serde_json::from_str(&std::fs::read_to_string(settings_path(&home)).unwrap()).unwrap();
        assert!(s["hooks"]["PreToolUse"].as_array().unwrap().len() >= 2);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn instalar_duas_vezes_nao_duplica_nada() {
        let home = temp();
        install(&home, Path::new("/opt/bilro"), None).unwrap();
        let r = install(&home, Path::new("/opt/bilro"), None).unwrap();
        assert!(r.added.is_empty(), "nao pode adicionar de novo: {:?}", r.added);
        assert_eq!(r.already.len(), 4);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn caminho_antigo_do_binario_e_atualizado_nao_duplicado() {
        let home = temp();
        install(&home, Path::new("/caminho/antigo/bilro"), None).unwrap();
        let r = install(&home, Path::new("/caminho/novo/bilro"), None).unwrap();
        assert_eq!(r.replaced.len(), 4, "deveria substituir: {:?}", r.replaced);
        let s: Value = serde_json::from_str(&std::fs::read_to_string(settings_path(&home)).unwrap()).unwrap();
        let txt = s.to_string();
        assert!(!txt.contains("/caminho/antigo/"), "sobrou o caminho antigo");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn nao_duplica_quando_o_hook_vive_em_outro_grupo() {
        let home = temp();
        std::fs::write(
            settings_path(&home),
            json!({"hooks":{"SessionStart":[
                {"hooks":[{"type":"command","command":"outra coisa"}]},
                {"hooks":[{"type":"command","command":"/opt/bilro hook session"}]}
            ]}})
            .to_string(),
        )
        .unwrap();
        let r = install(&home, Path::new("/opt/bilro"), None).unwrap();
        assert!(r.already.contains(&"SessionStart/*".to_string()), "deveria ver que ja existe: {r:?}", r = r.added);
        let s: Value = serde_json::from_str(&std::fs::read_to_string(settings_path(&home)).unwrap()).unwrap();
        let n = s["hooks"]["SessionStart"].as_array().unwrap().iter()
            .flat_map(|g| g["hooks"].as_array().unwrap())
            .filter(|h| h["command"].as_str() == Some("/opt/bilro hook session"))
            .count();
        assert_eq!(n, 1, "registrou o mesmo hook duas vezes");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn preserva_hook_de_outra_ferramenta() {
        let home = temp();
        std::fs::write(
            settings_path(&home),
            json!({"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"outra-ferramenta rodar"}]}]}}).to_string(),
        )
        .unwrap();
        install(&home, Path::new("/opt/bilro"), None).unwrap();
        let s = std::fs::read_to_string(settings_path(&home)).unwrap();
        assert!(s.contains("outra-ferramenta rodar"), "apagou hook de terceiro");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn guarda_copia_do_settings_que_alterou() {
        let home = temp();
        std::fs::write(settings_path(&home), "{}").unwrap();
        let r = install(&home, Path::new("/opt/bilro"), None).unwrap();
        assert!(r.backup.is_some_and(|b| b.exists()), "sem backup");
        std::fs::remove_dir_all(&home).ok();
    }
}
