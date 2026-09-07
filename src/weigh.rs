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
        let dir = std::env::temp_dir().join(format!("bilro-weigh-{label}-{}", uniq()));
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
    fn tokens_of_arredonda_bytes_sobre_quatro() {
        assert_eq!(tokens_of("12345678"), 2);
        assert_eq!(tokens_of("123456789"), 2);
        assert_eq!(tokens_of(""), 0);
    }

    #[test]
    fn always_on_pega_so_arquivos_que_existem() {
        let dir = tempdir("always-on");
        write(&dir.join("CLAUDE.md"), "0123456789abcdef");
        let items = weigh_always_on(&dir);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "CLAUDE.md");
        assert_eq!(items[0].tokens, 4);
    }

    #[test]
    fn memory_conta_entradas_e_ignora_backup_e_o_proprio_indice() {
        let root = tempdir("memory");
        let slug = "-Users-igorbrandao-Desktop-threadline";
        write(&root.join(slug).join("memory").join("MEMORY.md"), "# Memoria\n- [a](a.md)\n- [b](b.md)\ntexto solto\n");
        write(&root.join(slug).join("memory").join("a.md"), "conteudo a");
        write(&root.join(slug).join("memory").join("b.md"), "conteudo b");
        write(&root.join(slug).join("memory").join("a.bak-old.md"), "lixo");

        let out = weigh_memory(&root);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].project, slug);
        assert_eq!(out[0].entries, 2);
        assert_eq!(out[0].files, 2);
    }

    #[test]
    fn memory_ignora_projeto_sem_index() {
        let root = tempdir("memory-sem-index");
        write(&root.join("-outro-projeto").join("memory").join("nota.md"), "sem indice aqui");
        assert!(weigh_memory(&root).is_empty());
    }

    #[test]
    fn agents_marca_heranca_quando_falta_tools_e_deteta_context_mode() {
        let dir = tempdir("agents");
        write(&dir.join("com-tools.md"), "---\nname: com-tools\ndescription: usa ctx_execute\ntools: Read, Bash\n---\nusa ctx_execute pra tudo");
        write(&dir.join("sem-tools.md"), "---\nname: sem-tools\ndescription: agente generico\n---\nfaz grep cru");

        let agents = weigh_agents(&[dir]);
        assert_eq!(agents.len(), 2);
        let com = agents.iter().find(|a| a.name == "com-tools").unwrap();
        let sem = agents.iter().find(|a| a.name == "sem-tools").unwrap();
        assert!(!com.inherits_everything);
        assert!(com.has_context_mode);
        assert!(sem.inherits_everything);
        assert!(!sem.has_context_mode);
    }

    #[test]
    fn mcp_le_servidores_dos_dois_arquivos() {
        let home = tempdir("mcp");
        write(&home.join(".claude.json"), r#"{"mcpServers":{"context-mode":{}}}"#);
        write(&home.join(".claude").join("settings.json"), r#"{"mcpServers":{"facebook-ads-library":{}}}"#);
        let out = weigh_mcp(&home);
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|m| m.name == "context-mode"));
        assert!(out.iter().any(|m| m.name == "facebook-ads-library"));
    }

    #[test]
    fn plugins_le_chaves_do_installed_plugins() {
        let root = tempdir("plugins");
        write(&root.join("installed_plugins.json"), r#"{"plugins":{"ruflo":{"version":"1"}}}"#);
        let out = weigh_plugins(&root);
        assert_eq!(out, vec!["ruflo".to_string()]);
    }

    #[test]
    fn plugins_vazio_quando_arquivo_nao_existe() {
        let root = tempdir("plugins-vazio");
        assert!(weigh_plugins(&root).is_empty());
    }

    #[test]
    fn contra_arquivos_reais_claude_md_do_usuario() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let items = weigh_always_on(&home.join(".claude"));
        let claude = items.iter().find(|i| i.name == "CLAUDE.md");
        if let Some(c) = claude {
            let real = read_or(&home.join(".claude").join("CLAUDE.md"), "");
            assert_eq!(c.tokens, tokens_of(&real));
            assert!(c.tokens > 0);
        }
    }

    #[test]
    fn contra_arquivos_reais_memoria_do_projeto_threadline() {
        let root = PathBuf::from(std::env::var("HOME").unwrap()).join(".claude").join("projects");
        let out = weigh_memory(&root);
        let slug = "-Users-igorbrandao-Desktop-threadline";
        if let Some(m) = out.iter().find(|m| m.project == slug) {
            let real = read_or(&root.join(slug).join("memory").join("MEMORY.md"), "");
            assert_eq!(m.tokens, tokens_of(&real));
            assert!(m.entries > 0);
        }
    }
}
