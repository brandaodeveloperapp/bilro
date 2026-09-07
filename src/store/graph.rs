use crate::store::memory::{self, Memory};
use once_cell::sync::Lazy;
use regex::Regex;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::Path;

static LINK: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\[([^\]|#]+)(?:#[^\]|]+)?(?:\|[^\]]+)?\]\]").unwrap());

/// Names of memories a body links to, via `[[name]]`, `[[name#section]]`,
/// `[[name|alias]]` or `[[name#section|alias]]`, deduped in first-seen order.
pub fn links_of(body: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for cap in LINK.captures_iter(body) {
        let name = cap[1].trim().to_string();
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

/// Who points at whom, so a note can be judged by its place in the web.
pub struct Graph {
    pub memories: Vec<Memory>,
    pub names: HashSet<String>,
    pub out: HashMap<String, Vec<String>>,
    pub back: HashMap<String, Vec<String>>,
}

/// Builds the link graph for every memory in `dir`.
pub fn build(dir: &Path) -> Graph {
    let memories = memory::load_memories(dir);
    let names: HashSet<String> = memories.iter().map(|m| m.name.clone()).collect();
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut back: HashMap<String, Vec<String>> = HashMap::new();
    for m in &memories {
        let targets = links_of(&m.body);
        for t in &targets {
            back.entry(t.clone()).or_default().push(m.name.clone());
        }
        out.insert(m.name.clone(), targets);
    }
    Graph { memories, names, out, back }
}

/// A link that points at a memory with no matching name.
pub struct Broken {
    pub from: String,
    pub to: String,
}

/// A memory cited four times or more.
pub struct Hub {
    pub name: String,
    pub incoming: usize,
}

/// Result of a lint pass: broken links, orphan memories, and top hubs.
pub struct LintResult {
    pub total: usize,
    pub broken: Vec<Broken>,
    pub orphans: Vec<String>,
    pub hubs: Vec<Hub>,
}

/// Reports broken links, orphan memories, and the most cited memories.
pub fn lint(dir: &Path) -> LintResult {
    let g = build(dir);
    let mut broken = Vec::new();
    for m in &g.memories {
        if let Some(targets) = g.out.get(&m.name) {
            for t in targets {
                if !g.names.contains(t) {
                    broken.push(Broken { from: m.name.clone(), to: t.clone() });
                }
            }
        }
    }

    let mut orphans = Vec::new();
    let mut hubs = Vec::new();
    for m in &g.memories {
        let incoming = g.back.get(&m.name).map(|v| v.len()).unwrap_or(0);
        let outgoing = g.out.get(&m.name).map(|v| v.len()).unwrap_or(0);
        if incoming == 0 && outgoing == 0 {
            orphans.push(m.name.clone());
        }
        if incoming >= 4 {
            hubs.push(Hub { name: m.name.clone(), incoming });
        }
    }
    hubs.sort_by_key(|h| Reverse(h.incoming));
    hubs.truncate(5);

    LintResult { total: g.memories.len(), broken, orphans, hubs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn uniq_name() -> String {
        static N: AtomicU64 = AtomicU64::new(0);
        format!("t{}", N.fetch_add(1, Ordering::SeqCst))
    }

    fn vault(files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("bilro-graph-{}-{}", std::process::id(), uniq_name()));
        fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            let stem = name.strip_suffix(".md").unwrap_or(name);
            let content = format!("---\nname: {stem}\n---\n{body}");
            fs::write(dir.join(name), content).unwrap();
        }
        dir
    }

    #[test]
    fn le_as_quatro_formas_de_link() {
        let body = "ver [[b]], [[c|apelido]], [[d#secao]] e [[e#s|x]]";
        let mut found = links_of(body);
        found.sort();
        assert_eq!(found, vec!["b", "c", "d", "e"]);
    }

    #[test]
    fn acusa_link_para_memoria_inexistente() {
        let dir = vault(&[("a.md", "aponta pra [[sumida]]")]);
        let r = lint(&dir);
        assert_eq!(r.broken.len(), 1);
        assert_eq!(r.broken[0].to, "sumida");
    }

    #[test]
    fn link_valido_nao_vira_quebrado() {
        let dir = vault(&[("a.md", "vai pra [[b]]"), ("b.md", "fim")]);
        assert_eq!(lint(&dir).broken.len(), 0);
    }

    #[test]
    fn memoria_sem_link_entrando_nem_saindo_e_orfa() {
        let dir = vault(&[
            ("a.md", "vai pra [[b]]"),
            ("b.md", "fim"),
            ("sozinha.md", "ninguem me cita"),
        ]);
        assert_eq!(lint(&dir).orphans, vec!["sozinha".to_string()]);
    }

    #[test]
    fn memoria_muito_citada_aparece_como_hub() {
        let mut files: Vec<(String, String)> = vec![("centro.md".into(), "sou o centro".into())];
        for i in 0..5 {
            files.push((format!("n{i}.md"), "olha [[centro]]".into()));
        }
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let dir = vault(&refs);
        let r = lint(&dir);
        assert_eq!(r.hubs[0].name, "centro");
        assert_eq!(r.hubs[0].incoming, 5);
    }
}
