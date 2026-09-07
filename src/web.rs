use once_cell::sync::Lazy;
use regex::Regex;
use std::process::Command;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const TIMEOUT_SECS: u64 = 30;

static SCRIPT_OR_STYLE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?is)<script\b[^>]*>.*?</script\s*>|<style\b[^>]*>.*?</style\s*>|<noscript\b[^>]*>.*?</noscript\s*>|<svg\b[^>]*>.*?</svg\s*>|<!--.*?-->",
    )
    .unwrap()
});
static BLOCK_TAG: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)</?(p|div|br|li|tr|h[1-6]|section|article|header|footer|blockquote|pre)\b[^>]*>").unwrap()
});
static ANY_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<[^>]+>").unwrap());
static BLANK_RUN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n[ \t]*\n([ \t]*\n)+").unwrap());
static TRAILING: Lazy<Regex> = Lazy::new(|| Regex::new(r"[ \t]+\n").unwrap());
static TITLE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());

fn entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
}

/// Turns a page into the text a reader would actually take from it. Markup is
/// most of a page's bytes and almost none of its meaning.
pub fn to_text(html: &str) -> String {
    let s = SCRIPT_OR_STYLE.replace_all(html, " ");
    let s = BLOCK_TAG.replace_all(&s, "\n");
    let s = ANY_TAG.replace_all(&s, " ");
    let s = entities(&s);
    let s: String = s
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");
    let s = TRAILING.replace_all(&s, "\n");
    BLANK_RUN.replace_all(&s, "\n\n").trim().to_string()
}

pub fn title_of(html: &str) -> Option<String> {
    TITLE.captures(html).map(|c| entities(c[1].trim()).to_string()).filter(|t| !t.is_empty())
}

/// Only http and https. The model chooses these addresses, and a scheme like
/// file:// would turn a fetch into a way to read anything on the machine.
pub fn is_allowed_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://")) && !lower.contains(char::is_whitespace)
}

pub struct Page {
    pub url: String,
    pub title: Option<String>,
    pub text: String,
    pub bytes: usize,
}

/// Fetches a page and returns its readable text. curl does the transport, which
/// keeps a TLS stack and a certificate store out of this binary.
pub fn fetch(url: &str) -> Result<Page, String> {
    if !is_allowed_url(url) {
        return Err(format!("so http e https sao aceitos: {url}"));
    }
    let out = Command::new("curl")
        .args([
            "-sSL",
            "--max-time",
            &TIMEOUT_SECS.to_string(),
            "--max-filesize",
            &MAX_BYTES.to_string(),
            "-A",
            "bilro",
            url,
        ])
        .output()
        .map_err(|e| format!("curl indisponivel: {e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("busca falhou: {}", err.trim()));
    }
    let body = String::from_utf8_lossy(&out.stdout).to_string();
    let bytes = body.len();
    let looks_html = body.contains("<html") || body.contains("<body") || body.contains("<div");
    let text = if looks_html { to_text(&body) } else { body.clone() };
    Ok(Page {
        url: url.to_string(),
        title: title_of(&body),
        text: crate::redact::redact(&text),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recusa_esquema_que_nao_e_web() {
        for u in ["file:///etc/passwd", "gopher://x", "ftp://x", "javascript:alert(1)", "/etc/passwd", ""] {
            assert!(!is_allowed_url(u), "deveria recusar: {u}");
        }
        assert!(is_allowed_url("https://exemplo.com/a"));
        assert!(is_allowed_url("http://localhost:3000/x"));
    }

    #[test]
    fn extrai_o_texto_e_descarta_a_marcacao() {
        let html = "<html><head><title>Titulo</title><style>body{color:red}</style></head>\
                    <body><h1>Cabecalho</h1><p>Primeiro paragrafo.</p>\
                    <script>var x=1;</script><p>Segundo paragrafo.</p></body></html>";
        let t = to_text(html);
        assert!(t.contains("Cabecalho"));
        assert!(t.contains("Primeiro paragrafo."));
        assert!(t.contains("Segundo paragrafo."));
        assert!(!t.contains("color:red"), "sobrou css");
        assert!(!t.contains("var x"), "sobrou script");
        assert!(!t.contains('<'), "sobrou tag");
    }

    #[test]
    fn titulo_e_lido_quando_existe() {
        assert_eq!(title_of("<title> Guia do bilro </title>").as_deref(), Some("Guia do bilro"));
        assert_eq!(title_of("<p>sem titulo</p>"), None);
    }

    #[test]
    fn entidades_viram_caracteres() {
        assert_eq!(to_text("<p>a &amp; b &lt;c&gt; &quot;d&quot;</p>"), "a & b <c> \"d\"");
    }

    #[test]
    fn corta_muito_e_ainda_assim_encolhe_de_verdade() {
        let html = format!(
            "<html><body>{}</body></html>",
            "<div class=\"muito longa classe utilitaria aqui\"><span>texto</span></div>".repeat(300)
        );
        let t = to_text(&html);
        assert!(t.len() < html.len() / 4, "cortou pouco: {} -> {}", html.len(), t.len());
        assert!(t.contains("texto"));
    }

    #[test]
    fn texto_puro_passa_sem_estrago() {
        let plano = "linha um\nlinha dois\n\nlinha quatro";
        assert_eq!(to_text(plano), plano);
    }
}
