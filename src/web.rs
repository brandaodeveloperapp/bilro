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
        return Err(format!("only http and https are accepted: {url}"));
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
        .map_err(|e| format!("curl unavailable: {e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("fetch failed: {}", err.trim()));
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
    fn refuses_a_non_web_scheme() {
        for u in ["file:///etc/passwd", "gopher://x", "ftp://x", "javascript:alert(1)", "/etc/passwd", ""] {
            assert!(!is_allowed_url(u), "should refuse: {u}");
        }
        assert!(is_allowed_url("https://example.com/a"));
        assert!(is_allowed_url("http://localhost:3000/x"));
    }

    #[test]
    fn extracts_the_text_and_discards_the_markup() {
        let html = "<html><head><title>Title</title><style>body{color:red}</style></head>\
                    <body><h1>Heading</h1><p>First paragraph.</p>\
                    <script>var x=1;</script><p>Second paragraph.</p></body></html>";
        let t = to_text(html);
        assert!(t.contains("Heading"));
        assert!(t.contains("First paragraph."));
        assert!(t.contains("Second paragraph."));
        assert!(!t.contains("color:red"), "css survived");
        assert!(!t.contains("var x"), "script survived");
        assert!(!t.contains('<'), "a tag survived");
    }

    #[test]
    fn title_is_read_when_present() {
        assert_eq!(title_of("<title> bilro guide </title>").as_deref(), Some("bilro guide"));
        assert_eq!(title_of("<p>no title</p>"), None);
    }

    #[test]
    fn entities_become_characters() {
        assert_eq!(to_text("<p>a &amp; b &lt;c&gt; &quot;d&quot;</p>"), "a & b <c> \"d\"");
    }

    #[test]
    fn cutting_a_lot_still_actually_shrinks() {
        let html = format!(
            "<html><body>{}</body></html>",
            "<div class=\"very long utility class here\"><span>text</span></div>".repeat(300)
        );
        let t = to_text(&html);
        assert!(t.len() < html.len() / 4, "cut too little: {} -> {}", html.len(), t.len());
        assert!(t.contains("text"));
    }

    #[test]
    fn plain_text_passes_through_undamaged() {
        let plain = "line one\nline two\n\nline four";
        assert_eq!(to_text(plain), plain);
    }
}
