use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{params, Connection};
use std::collections::HashSet;

static FAILURE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(error|erro|failed|falhou|fatal|exception|traceback|refused|denied|not found|cannot|timeout)\b")
        .unwrap()
});

/// A recipe worth writing down (a command run many times) or a trap worth
/// remembering (a failure line seen across several runs).
pub struct Proposal {
    pub kind: &'static str,
    pub subject: String,
    pub seen: i64,
    pub why: String,
}

/// Knobs for `proposals`: how many runs make a recipe, how many make a trap.
pub struct ProposalOptions {
    pub min_runs: i64,
    pub min_failure_runs: i64,
}

impl Default for ProposalOptions {
    fn default() -> Self {
        Self { min_runs: 5, min_failure_runs: 2 }
    }
}

/// A memory file the owner only has to edit, not write from nothing.
pub struct Draft {
    pub slug: String,
    pub content: String,
}

fn strip_diacritic(c: char) -> char {
    match c {
        'á' | 'à' | 'ã' | 'â' | 'ä' | 'å' => 'a',
        'Á' | 'À' | 'Ã' | 'Â' | 'Ä' | 'Å' => 'A',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
        'ó' | 'ò' | 'õ' | 'ô' | 'ö' => 'o',
        'Ó' | 'Ò' | 'Õ' | 'Ô' | 'Ö' => 'O',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
        'ç' => 'c',
        'Ç' => 'C',
        'ñ' => 'n',
        'Ñ' => 'N',
        other => other,
    }
}

fn slugify(subject: &str) -> String {
    let lowered: String =
        subject.chars().map(strip_diacritic).collect::<String>().to_lowercase();
    let mut slug = String::new();
    let mut prev_dash = false;
    for c in lowered.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            slug.push(c);
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    slug.trim_matches('-').chars().take(48).collect()
}

/// A command shape run many times is a recipe someone had to rediscover; a
/// failure line seen across several runs is a trap that will be stepped on
/// again. Both are memories waiting to be written.
pub fn proposals(db: &Connection, opts: &ProposalOptions) -> rusqlite::Result<Vec<Proposal>> {
    let mut out = Vec::new();

    {
        let mut st = db.prepare("SELECT sig, n FROM runs WHERE n >= ?1 ORDER BY n DESC LIMIT 12")?;
        let rows = st.query_map(params![opts.min_runs], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (sig, n) = row?;
            out.push(Proposal {
                kind: "receita",
                subject: sig,
                seen: n,
                why: format!("rodado {n} vezes — vale virar receita com `verify:`"),
            });
        }
    }

    let mut candidates: Vec<(String, i64, Option<String>)> = Vec::new();
    {
        let mut st = db.prepare(
            "SELECT l.sig, l.df, x.body FROM lines l
             JOIN exact e ON e.sig = l.sig AND e.shape = l.h
             LEFT JOIN last x ON x.sig = l.sig
             WHERE l.df >= ?1 ORDER BY l.df DESC LIMIT 200",
        )?;
        let rows = st.query_map(params![opts.min_failure_runs], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?))
        })?;
        for row in rows {
            candidates.push(row?);
        }
    }

    let mut seen_lines: HashSet<String> = HashSet::new();
    let mut trap_count = 0;
    for (sig, df, body) in candidates {
        let Some(body) = body else { continue };
        let Some(hit) = body.split('\n').find(|l| FAILURE.is_match(l) && l.trim().len() > 15)
        else {
            continue;
        };
        if seen_lines.contains(hit) {
            continue;
        }
        seen_lines.insert(hit.to_string());
        let subject: String = hit.trim().chars().take(120).collect();
        let sig_short: String = sig.chars().take(40).collect();
        out.push(Proposal {
            kind: "armadilha",
            subject,
            seen: df,
            why: format!("apareceu em {df} execucoes de `{sig_short}`"),
        });
        trap_count += 1;
        if trap_count >= 6 {
            break;
        }
    }

    Ok(out)
}

/// Turns a proposal into a ready-to-edit memory file.
pub fn draft(proposal: &Proposal) -> Draft {
    let slug = slugify(&proposal.subject);
    let body = if proposal.kind == "receita" {
        format!(
            "Comando rodado {} vezes:\n\n    {}\n\nEscreva aqui o que ele resolve e o que quebra quando falha.",
            proposal.seen, proposal.subject
        )
    } else {
        format!(
            "Falha vista em {} execucoes:\n\n    {}\n\nEscreva aqui a causa e o conserto, para nao redescobrir.",
            proposal.seen, proposal.subject
        )
    };
    let content = format!(
        "---\nname: {slug}\ndescription: {}\nmetadata:\n  type: project\n---\n\n{body}\n",
        proposal.why
    );
    Draft { slug, content }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::learn::{observe, open};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn uniq_name() -> String {
        static N: AtomicU64 = AtomicU64::new(0);
        format!("t{}", N.fetch_add(1, Ordering::SeqCst))
    }

    fn db() -> Connection {
        let dir = std::env::temp_dir().join(format!("bilro-propose-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file: PathBuf = dir.join(format!("{}.db", uniq_name()));
        open(&file).unwrap()
    }

    #[test]
    fn comando_pouco_usado_nao_vira_proposta() {
        let mut d = db();
        observe(&mut d, "npx jest a", "tudo ok").unwrap();
        assert_eq!(proposals(&d, &ProposalOptions::default()).unwrap().len(), 0);
    }

    #[test]
    fn comando_repetido_vira_receita() {
        let mut d = db();
        for _ in 0..6 {
            observe(&mut d, "npx jest a", "tudo ok").unwrap();
        }
        let r = proposals(&d, &ProposalOptions::default()).unwrap();
        assert_eq!(r[0].kind, "receita");
        assert_eq!(r[0].seen, 6);
    }

    #[test]
    fn linha_de_falha_recorrente_vira_armadilha() {
        let mut d = db();
        for _ in 0..3 {
            observe(&mut d, "deploy prod", "subindo\nError: connection refused pelo cluster remoto").unwrap();
        }
        let r = proposals(&d, &ProposalOptions::default()).unwrap();
        assert!(r.iter().any(|p| p.kind == "armadilha" && p.subject.contains("connection refused")));
    }

    #[test]
    fn saida_saudavel_nao_vira_armadilha() {
        let mut d = db();
        for _ in 0..6 {
            observe(&mut d, "build ok", "compilado com sucesso\ntudo certo por aqui").unwrap();
        }
        let r = proposals(&d, &ProposalOptions::default()).unwrap();
        assert_eq!(r.iter().filter(|p| p.kind == "armadilha").count(), 0);
    }

    #[test]
    fn rascunho_sai_com_frontmatter_valido_e_slug_utilizavel() {
        let d = draft(&Proposal {
            kind: "armadilha",
            subject: "Erro: conexão RECUSADA no cluster".to_string(),
            seen: 3,
            why: "x".to_string(),
        });
        assert!(Regex::new("^[a-z0-9-]+$").unwrap().is_match(&d.slug));
        assert!(d.content.starts_with("---\nname: "));
        assert!(d.content.contains("type: project"));
    }
}
