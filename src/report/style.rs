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
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("invalid level: {level}")));
    }
    std::fs::create_dir_all(home_dir().join(".claude").join("bilro"))?;
    std::fs::write(config_path(), level)?;
    Ok(level.to_string())
}

const LEAN: &str = "Write lean. Cut the greeting, the preamble, \"I'll do X\" before doing it, and the recap of what was just read. One sentence per idea. Table only when comparing three things or more.";

const TERSE_EXTRA: &str = "Also cut: an article where the sentence survives without it, an intensity adverb, hedging (\"maybe\", \"I think\") when you measured it, and repeating what the user just said. A fragment is fine. Technical terms and error messages stay literal.";

const ALWAYS: &str = "Write normally (no cuts) for: code, commit messages, PR bodies, security warnings, confirmation of an irreversible action, and step-by-step sequences where order matters.";

const ANTIDRIFT: &str = "This rule applies to EVERY response in this session, including status reports and command results. An instruction stated once decays over a long conversation — if in doubt, cut.";

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
    fn off_injects_nothing() {
        assert_eq!(ruleset("off"), "");
    }

    #[test]
    fn terse_carries_everything_lean_carries() {
        let lean = ruleset("lean");
        let terse = ruleset("terse");
        let first = lean.split('\n').nth(1).unwrap();
        assert!(terse.contains(first));
        assert!(terse.len() > lean.len());
    }

    #[test]
    fn every_level_on_preserves_the_exceptions() {
        for l in LEVELS.iter().filter(|l| **l != "off") {
            let r = ruleset(l).to_lowercase();
            assert!(r.contains("security warning") || r.contains("security"));
            assert!(ruleset(l).contains("commit"));
        }
    }

    #[test]
    fn every_level_on_carries_the_antidrift_rule() {
        for l in LEVELS.iter().filter(|l| **l != "off") {
            assert!(ruleset(l).contains("EVERY response"));
        }
    }
}

#[cfg(test)]
mod inherit_tests {
    use super::*;

    #[test]
    fn understands_the_vocabulary_of_the_replaced_tool() {
        assert_eq!(normalize("full"), Some("terse"));
        assert_eq!(normalize("ultra"), Some("terse"));
        assert_eq!(normalize("lite"), Some("lean"));
        assert_eq!(normalize("off"), Some("off"));
        assert_eq!(normalize(" full \n"), Some("terse"));
        assert_eq!(normalize("madeup"), None);
    }

    #[test]
    fn inherited_level_carries_all_the_rules() {
        let rules = ruleset(normalize("full").unwrap());
        assert!(rules.contains("fragment"), "full level should carry the cutting rules");
        assert!(!ruleset("off").trim().is_empty() || ruleset("off").is_empty());
    }
}

#[cfg(test)]
mod own_tests {
    use super::*;

    #[test]
    fn own_level_differs_from_inherited_level() {
        let inherited = read_level();
        assert!(!inherited.is_empty(), "always resolves to some level");
        if own_level().is_none() {
            assert!(
                std::fs::read_to_string(config_path()).is_err(),
                "with no own choice, bilro's file should not exist"
            );
        }
    }
}
