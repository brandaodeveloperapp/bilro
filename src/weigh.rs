use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const CHARS_PER_TOKEN: f64 = 4.0;

static ENTRY_LINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^- \[").unwrap());
static FRONTMATTER_TOOLS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^tools:\s*(.+)$").unwrap());
static FRONTMATTER_DESC: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^description:\s*(.+)$").unwrap());
static HAS_CONTEXT_MODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"ctx_batch_execute|ctx_execute|ctx_search").unwrap());

pub fn tokens_of(text: &str) -> i64 {
    (text.chars().count() as f64 / CHARS_PER_TOKEN).round() as i64
}

fn read_or(path: &Path, fallback: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|_| fallback.to_string())
}

fn list_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(ext)))
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    files
}

pub struct AlwaysOnItem {
    pub name: String,
    pub tokens: i64,
    pub path: PathBuf,
}

/// Files injected into the system prompt of every request.
pub fn weigh_always_on(root: &Path) -> Vec<AlwaysOnItem> {
    let mut items = Vec::new();
    for name in ["CLAUDE.md", "RTK.md"] {
        let path = root.join(name);
        if path.exists() {
            items.push(AlwaysOnItem { name: name.to_string(), tokens: tokens_of(&read_or(&path, "")), path });
        }
    }
    items
}

pub struct MemoryItem {
    pub project: String,
    pub tokens: i64,
    pub entries: usize,
    pub files: usize,
    pub path: PathBuf,
}

/// Per-project memory index, injected on every request in that project.
pub fn weigh_memory(root: &Path) -> Vec<MemoryItem> {
    let mut out = Vec::new();
    let projects: Vec<String> = match std::fs::read_dir(root) {
        Ok(entries) => entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect(),
        Err(_) => return out,
    };
    for project in projects {
        let index = root.join(&project).join("memory").join("MEMORY.md");
        if !index.exists() {
            continue;
        }
        let memory_dir = root.join(&project).join("memory");
        let files: Vec<PathBuf> = list_files(&memory_dir, ".md")
            .into_iter()
            .filter(|f| {
                let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name != "MEMORY.md" && !name.contains(".bak-")
            })
            .collect();
        let text = read_or(&index, "");
        out.push(MemoryItem {
            project,
            tokens: tokens_of(&text),
            entries: ENTRY_LINE.find_iter(&text).count(),
            files: files.len(),
            path: index,
        });
    }
    out
}

fn frontmatter_tools(text: &str) -> Option<String> {
    FRONTMATTER_TOOLS.captures(text).map(|c| c[1].trim().to_string())
}

pub struct AgentItem {
    pub name: String,
    pub dir: PathBuf,
    pub catalogue_tokens: i64,
    pub inherits_everything: bool,
    pub has_context_mode: bool,
    pub path: PathBuf,
}

/// Agent definitions: name+description ride in every request, and an agent
/// with no explicit `tools:` inherits the entire tool catalogue at dispatch.
pub fn weigh_agents(dirs: &[PathBuf]) -> Vec<AgentItem> {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, AgentItem> = HashMap::new();
    for dir in dirs {
        for path in list_files(dir, ".md") {
            let text = read_or(&path, "");
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let tools = frontmatter_tools(&text);
            let desc = FRONTMATTER_DESC.captures(&text).map(|c| c[1].to_string()).unwrap_or_default();
            let key = format!("{}:{}", dir.display(), name);
            if !map.contains_key(&key) {
                order.push(key.clone());
            }
            map.insert(
                key,
                AgentItem {
                    catalogue_tokens: tokens_of(&format!("{name} {desc}")),
                    inherits_everything: tools.is_none(),
                    has_context_mode: HAS_CONTEXT_MODE.is_match(&text),
                    name,
                    dir: dir.clone(),
                    path,
                },
            );
        }
    }
    order.into_iter().filter_map(|k| map.remove(&k)).collect()
}

pub struct McpItem {
    pub name: String,
    pub source: String,
}

/// MCP servers declared for the CLI — every tool schema loads per request.
pub fn weigh_mcp(home: &Path) -> Vec<McpItem> {
    let mut out = Vec::new();
    for file in [home.join(".claude.json"), home.join(".claude").join("settings.json")] {
        let text = read_or(&file, "{}");
        let Ok(cfg) = serde_json::from_str::<Value>(&text) else { continue };
        let Some(servers) = cfg.get("mcpServers").and_then(|s| s.as_object()) else { continue };
        let source = file.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        for name in servers.keys() {
            out.push(McpItem { name: name.clone(), source: source.clone() });
        }
    }
    out
}

pub fn weigh_plugins(root: &Path) -> Vec<String> {
    let text = read_or(&root.join("installed_plugins.json"), "{}");
    match serde_json::from_str::<Value>(&text) {
        Ok(cfg) => cfg.get("plugins").and_then(|p| p.as_object()).map(|m| m.keys().cloned().collect()).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

pub fn disk_usage(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tempdir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("bilro-weigh-{label}-{}-{}", std::process::id(), uniq()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn uniq() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::SeqCst)
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn tokens_of_rounds_bytes_over_four() {
        assert_eq!(tokens_of("12345678"), 2);
        assert_eq!(tokens_of("123456789"), 2);
        assert_eq!(tokens_of(""), 0);
    }

    #[test]
    fn always_on_only_picks_up_files_that_exist() {
        let dir = tempdir("always-on");
        write(&dir.join("CLAUDE.md"), "0123456789abcdef");
        let items = weigh_always_on(&dir);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "CLAUDE.md");
        assert_eq!(items[0].tokens, 4);
    }

    #[test]
    fn memory_counts_entries_and_ignores_backups_and_its_own_index() {
        let root = tempdir("memory");
        let slug = "-home-dev-example";
        write(&root.join(slug).join("memory").join("MEMORY.md"), "# Memory\n- [a](a.md)\n- [b](b.md)\nloose text\n");
        write(&root.join(slug).join("memory").join("a.md"), "content a");
        write(&root.join(slug).join("memory").join("b.md"), "content b");
        write(&root.join(slug).join("memory").join("a.bak-old.md"), "junk");

        let out = weigh_memory(&root);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].project, slug);
        assert_eq!(out[0].entries, 2);
        assert_eq!(out[0].files, 2);
    }

    #[test]
    fn memory_ignores_a_project_with_no_index() {
        let root = tempdir("memory-no-index");
        write(&root.join("-other-project").join("memory").join("note.md"), "no index here");
        assert!(weigh_memory(&root).is_empty());
    }

    #[test]
    fn agents_flags_inheritance_when_tools_is_missing_and_detects_context_mode() {
        let dir = tempdir("agents");
        write(&dir.join("with-tools.md"), "---\nname: with-tools\ndescription: uses ctx_execute\ntools: Read, Bash\n---\nuses ctx_execute for everything");
        write(&dir.join("without-tools.md"), "---\nname: without-tools\ndescription: generic agent\n---\ndoes raw grep");

        let agents = weigh_agents(&[dir]);
        assert_eq!(agents.len(), 2);
        let with = agents.iter().find(|a| a.name == "with-tools").unwrap();
        let without = agents.iter().find(|a| a.name == "without-tools").unwrap();
        assert!(!with.inherits_everything);
        assert!(with.has_context_mode);
        assert!(without.inherits_everything);
        assert!(!without.has_context_mode);
    }

    #[test]
    fn mcp_reads_servers_from_both_files() {
        let home = tempdir("mcp");
        write(&home.join(".claude.json"), r#"{"mcpServers":{"context-mode":{}}}"#);
        write(&home.join(".claude").join("settings.json"), r#"{"mcpServers":{"facebook-ads-library":{}}}"#);
        let out = weigh_mcp(&home);
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|m| m.name == "context-mode"));
        assert!(out.iter().any(|m| m.name == "facebook-ads-library"));
    }

    #[test]
    fn plugins_reads_keys_from_installed_plugins() {
        let root = tempdir("plugins");
        write(&root.join("installed_plugins.json"), r#"{"plugins":{"ruflo":{"version":"1"}}}"#);
        let out = weigh_plugins(&root);
        assert_eq!(out, vec!["ruflo".to_string()]);
    }

    #[test]
    fn plugins_is_empty_when_the_file_does_not_exist() {
        let root = tempdir("plugins-empty");
        assert!(weigh_plugins(&root).is_empty());
    }

    #[test]
    fn always_on_token_count_matches_an_independent_read_of_the_same_file() {
        let dir = tempdir("always-on-consistency");
        write(&dir.join("CLAUDE.md"), "some fixture content that is not a round number of chars");
        let items = weigh_always_on(&dir);
        let claude = items.iter().find(|i| i.name == "CLAUDE.md").unwrap();
        let reread = read_or(&dir.join("CLAUDE.md"), "");
        assert_eq!(claude.tokens, tokens_of(&reread));
        assert!(claude.tokens > 0);
    }

    #[test]
    fn memory_token_count_matches_an_independent_read_of_the_same_index() {
        let root = tempdir("memory-consistency");
        let slug = "-home-dev-example";
        let index = root.join(slug).join("memory").join("MEMORY.md");
        write(&index, "# Memory\n- [a](a.md)\n- [b](b.md)\n- [c](c.md)\nsome extra prose here too\n");

        let out = weigh_memory(&root);
        let m = out.iter().find(|m| m.project == slug).unwrap();
        let reread = read_or(&index, "");
        assert_eq!(m.tokens, tokens_of(&reread));
        assert!(m.entries > 0);
    }
}
