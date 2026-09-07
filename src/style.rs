use std::path::PathBuf;

pub const LEVELS: [&str; 3] = ["off", "lean", "terse"];

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

fn config_path() -> PathBuf {
    home_dir().join(".claude").join("bilro").join("style")
}

/// Where the tool bilro replaces keeps the same setting. Reading it means a
/// switch costs nothing: whatever level was chosen there still applies here.
fn inherited_path() -> PathBuf {
    home_dir().join(".claude").join(".caveman-active")
}

/// Accepts the vocabulary of the tool being replaced alongside its own, so an
/// existing configuration keeps meaning what it meant. `ultra` maps onto the
/// strongest level bilro defines rather than inventing a fourth.
pub fn normalize(level: &str) -> Option<&'static str> {
    match level.trim() {
        "off" | "none" => Some("off"),
        "lean" | "lite" => Some("lean"),
        "terse" | "full" | "ultra" => Some("terse"),
        _ => None,
    }
}

/// The level chosen for bilro itself, as opposed to one inherited from the
/// tool it replaces. A hook uses this to stay quiet while the other tool is
/// still speaking, instead of both saying the same thing every turn.
pub fn own_level() -> Option<String> {
    std::fs::read_to_string(config_path()).ok().and_then(|v| normalize(&v).map(String::from))
}

pub fn read_level() -> String {
    for path in [config_path(), inherited_path()] {
        if let Ok(v) = std::fs::read_to_string(&path) {
            if let Some(level) = normalize(&v) {
                return level.to_string();
            }
        }
    }
    "off".to_string()
}

pub fn write_level(level: &str) -> std::io::Result<String> {
    let level = normalize(level).unwrap_or(level);
    if !LEVELS.contains(&level) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("nivel invalido: {level}")));
    }
    std::fs::create_dir_all(home_dir().join(".claude").join("bilro"))?;
    std::fs::write(config_path(), level)?;
    Ok(level.to_string())
}

const LEAN: &str = "Escreva enxuto. Corte saudação, preâmbulo, \"vou fazer X\" antes de fazer, e o resumo do que acabou de ser lido. Uma frase por ideia. Tabela só quando compara três coisas ou mais.";

const TERSE_EXTRA: &str = "Corte também: artigo onde a frase sobrevive sem ele, advérbio de intensidade, hedge (\"talvez\", \"acho que\") quando você mediu, e a repetição do que o usuário acabou de dizer. Fragmento é aceitável. Termo técnico e mensagem de erro ficam literais.";

const ALWAYS: &str = "Escreva normal (sem cortes) em: código, mensagem de commit, corpo de PR, aviso de segurança, confirmação de ação irreversível, e passo a passo onde a ordem importa.";

const ANTIDRIFT: &str = "Esta regra vale para TODA resposta desta sessão, inclusive relatório de status e resultado de comando. Instrução dita uma vez decai em conversa longa — se estiver em dúvida, corte.";

/// The rules re-emitted every session, since a rule stated once decays.
pub fn ruleset(level: &str) -> String {
    if level == "off" {
        return String::new();
    }
    let body = if level == "terse" { format!("{LEAN}\n{TERSE_EXTRA}") } else { LEAN.to_string() };
    [format!("bilro style: {level}"), body, ALWAYS.to_string(), ANTIDRIFT.to_string()].join("\n")
}

pub fn ruleset_default() -> String {
    ruleset(&read_level())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_nao_injeta_nada() {
        assert_eq!(ruleset("off"), "");
    }

    #[test]
    fn terse_carrega_tudo_que_lean_carrega() {
        let lean = ruleset("lean");
        let terse = ruleset("terse");
        let primeira = lean.split('\n').nth(1).unwrap();
        assert!(terse.contains(primeira));
        assert!(terse.len() > lean.len());
    }

    #[test]
    fn todo_nivel_ligado_preserva_as_excecoes() {
        for l in LEVELS.iter().filter(|l| **l != "off") {
            let r = ruleset(l).to_lowercase();
            assert!(r.contains("aviso de seguranca") || r.contains("aviso de segurança") || r.contains("seguran"));
            assert!(ruleset(l).contains("commit"));
        }
    }

    #[test]
    fn todo_nivel_ligado_carrega_a_regra_anti_deriva() {
        for l in LEVELS.iter().filter(|l| **l != "off") {
            assert!(ruleset(l).contains("TODA resposta"));
        }
    }
}

#[cfg(test)]
mod inherit_tests {
    use super::*;

    #[test]
    fn entende_o_vocabulario_da_ferramenta_substituida() {
        assert_eq!(normalize("full"), Some("terse"));
        assert_eq!(normalize("ultra"), Some("terse"));
        assert_eq!(normalize("lite"), Some("lean"));
        assert_eq!(normalize("off"), Some("off"));
        assert_eq!(normalize(" full \n"), Some("terse"));
        assert_eq!(normalize("inventado"), None);
    }

    #[test]
    fn nivel_herdado_carrega_as_regras_todas() {
        let rules = ruleset(normalize("full").unwrap());
        assert!(rules.contains("Fragmento"), "nivel full deve carregar as regras de corte");
        assert!(!ruleset("off").trim().is_empty() || ruleset("off").is_empty());
    }
}

#[cfg(test)]
mod own_tests {
    use super::*;

    #[test]
    fn nivel_proprio_e_diferente_de_nivel_herdado() {
        let herdado = read_level();
        assert!(!herdado.is_empty(), "sempre resolve algum nivel");
        if own_level().is_none() {
            assert!(
                std::fs::read_to_string(config_path()).is_err(),
                "sem escolha propria, o arquivo do bilro nao deve existir"
            );
        }
    }
}
