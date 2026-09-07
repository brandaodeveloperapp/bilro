use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const HOOKS: &[(&str, Option<&str>, &str)] = &[
    ("PreToolUse", Some("Bash"), "hook bash"),
    ("PreToolUse", Some("Task"), "hook task"),
    ("PostToolUse", Some("Bash"), "hook shadow"),
    ("SessionStart", None, "hook session"),
    ("UserPromptSubmit", None, "hook prompt"),
    ("PostToolUse", Some("Task"), "hook outcome"),
    ("PostToolUse", Some("Edit"), "hook outcome"),
];

pub struct Report {
    pub mcp: Option<&'static str>,
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
        mcp: None,
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

        let same_matcher = |g: &Value| {
            g.get("matcher").and_then(|m| m.as_str()) == *matcher
                || (matcher.is_none() && g.get("matcher").is_none())
        };
        let already_registered = groups.iter().filter(|g| same_matcher(g)).any(|g| {
            g.get("hooks")
                .and_then(|h| h.as_array())
                .map(|arr| arr.iter().any(|h| h.get("command").and_then(|c| c.as_str()) == Some(command.as_str())))
                .unwrap_or(false)
        });
        if already_registered {
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
    report.mcp = register_mcp(home, binary)?;
    Ok(report)
}

/// Registers the stdio MCP server so the tools appear in the model's own list.
/// A capability that has to be remembered is a capability that goes unused.
fn register_mcp(home: &Path, binary: &Path) -> std::io::Result<Option<&'static str>> {
    let path = home.join(".claude.json");
    let mut root: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| json!({}));
    let Some(obj) = root.as_object_mut() else { return Ok(None) };
    let servers = obj.entry("mcpServers").or_insert_with(|| json!({}));
    let Some(servers) = servers.as_object_mut() else { return Ok(None) };

    let desired = json!({
        "type": "stdio",
        "command": binary.display().to_string(),
        "args": ["mcp"],
        "env": {}
    });
    let outcome = match servers.get("bilro") {
        Some(existing) if *existing == desired => Some("already there"),
        Some(_) => Some("path updated"),
        None => Some("registered"),
    };
    if servers.get("bilro") == Some(&desired) {
        return Ok(outcome);
    }
    servers.insert("bilro".into(), desired);
    std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
    Ok(outcome)
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
    fn registers_the_mcp_server() {
        let home = temp();
        install(&home, Path::new("/opt/bilro"), None).unwrap();
        let s: Value = serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(s["mcpServers"]["bilro"]["command"], "/opt/bilro");
        assert_eq!(s["mcpServers"]["bilro"]["args"][0], "mcp");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn does_not_erase_other_mcp_servers() {
        let home = temp();
        std::fs::write(
            home.join(".claude.json"),
            json!({"mcpServers":{"playwright":{"type":"stdio","command":"npx"}}}).to_string(),
        )
        .unwrap();
        install(&home, Path::new("/opt/bilro"), None).unwrap();
        let s: Value = serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
        assert_eq!(s["mcpServers"]["playwright"]["command"], "npx");
        assert!(s["mcpServers"]["bilro"].is_object());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn registers_all_hooks_on_a_clean_environment() {
        let home = temp();
        let r = install(&home, Path::new("/opt/bilro"), None).unwrap();
        assert_eq!(r.added.len(), 7, "added: {:?}", r.added);
        let s: Value = serde_json::from_str(&std::fs::read_to_string(settings_path(&home)).unwrap()).unwrap();
        assert!(s["hooks"]["PreToolUse"].as_array().unwrap().len() >= 2);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn installing_twice_does_not_duplicate_anything() {
        let home = temp();
        install(&home, Path::new("/opt/bilro"), None).unwrap();
        let r = install(&home, Path::new("/opt/bilro"), None).unwrap();
        assert!(r.added.is_empty(), "must not add again: {:?}", r.added);
        assert_eq!(r.already.len(), 7);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn old_binary_path_is_updated_not_duplicated() {
        let home = temp();
        install(&home, Path::new("/old/path/bilro"), None).unwrap();
        let r = install(&home, Path::new("/new/path/bilro"), None).unwrap();
        assert_eq!(r.replaced.len(), 7, "should replace: {:?}", r.replaced);
        let s: Value = serde_json::from_str(&std::fs::read_to_string(settings_path(&home)).unwrap()).unwrap();
        let txt = s.to_string();
        assert!(!txt.contains("/old/path/"), "old path survived");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn two_hooks_with_the_same_command_on_different_matchers_coexist() {
        let home = temp();
        let r = install(&home, Path::new("/opt/bilro"), None).unwrap();
        let s: Value = serde_json::from_str(&std::fs::read_to_string(settings_path(&home)).unwrap()).unwrap();
        let post = s["hooks"]["PostToolUse"].as_array().unwrap();
        let outcome_matchers: Vec<&str> = post
            .iter()
            .filter(|g| {
                g["hooks"].as_array().unwrap().iter().any(|h| h["command"].as_str().unwrap_or("").ends_with("hook outcome"))
            })
            .map(|g| g["matcher"].as_str().unwrap_or("*"))
            .collect();
        assert!(outcome_matchers.contains(&"Task"), "missing Task: {outcome_matchers:?} | added {:?}", r.added);
        assert!(outcome_matchers.contains(&"Edit"), "missing Edit: {outcome_matchers:?}");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn does_not_duplicate_when_the_hook_lives_in_another_group() {
        let home = temp();
        std::fs::write(
            settings_path(&home),
            json!({"hooks":{"SessionStart":[
                {"hooks":[{"type":"command","command":"something else"}]},
                {"hooks":[{"type":"command","command":"/opt/bilro hook session"}]}
            ]}})
            .to_string(),
        )
        .unwrap();
        let r = install(&home, Path::new("/opt/bilro"), None).unwrap();
        assert!(r.already.contains(&"SessionStart/*".to_string()), "should see it already exists: {r:?}", r = r.added);
        let s: Value = serde_json::from_str(&std::fs::read_to_string(settings_path(&home)).unwrap()).unwrap();
        let n = s["hooks"]["SessionStart"].as_array().unwrap().iter()
            .flat_map(|g| g["hooks"].as_array().unwrap())
            .filter(|h| h["command"].as_str() == Some("/opt/bilro hook session"))
            .count();
        assert_eq!(n, 1, "registered the same hook twice");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn preserves_another_tool_s_hook() {
        let home = temp();
        std::fs::write(
            settings_path(&home),
            json!({"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"other-tool run"}]}]}}).to_string(),
        )
        .unwrap();
        install(&home, Path::new("/opt/bilro"), None).unwrap();
        let s = std::fs::read_to_string(settings_path(&home)).unwrap();
        assert!(s.contains("other-tool run"), "erased a third party's hook");
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn keeps_a_copy_of_the_settings_it_changed() {
        let home = temp();
        std::fs::write(settings_path(&home), "{}").unwrap();
        let r = install(&home, Path::new("/opt/bilro"), None).unwrap();
        assert!(r.backup.is_some_and(|b| b.exists()), "no backup");
        std::fs::remove_dir_all(&home).ok();
    }
}
