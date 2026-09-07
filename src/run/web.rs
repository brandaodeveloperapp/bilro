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
/// The machine bilro runs on is the one place a fetched page must not reach:
/// the cloud metadata endpoint, the panel on 7777, every dev service on
/// localhost. A page bilro reads can carry instructions, so the URL the next
/// fetch uses is not necessarily one a person chose.
fn ip_is_internal(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        other => other,
    };
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.octets()[0] == 0
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

/// `127.0.0.1` is only the spelling everyone recognises. `2130706433`,
/// `0x7f000001`, `127.1`, `[::ffff:127.0.0.1]` and a hostname somebody pointed
/// at loopback all reach the same socket, so the question is answered by
/// resolving the name, not by reading it.
fn is_internal_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    if h.is_empty() || h.eq_ignore_ascii_case("localhost") || h.ends_with(".localhost") || h.ends_with(".internal") {
        return true;
    }
    if let Ok(ip) = h.parse::<std::net::IpAddr>() {
        return ip_is_internal(ip);
    }
    use std::net::ToSocketAddrs;
    match (h, 80u16).to_socket_addrs() {
        Ok(addrs) => {
            let mut any = false;
            for a in addrs {
                any = true;
                if ip_is_internal(a.ip()) {
                    return true;
                }
            }
            !any
        }
        Err(_) => true,
    }
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    match authority.rfind(':') {
        Some(i) if !authority[i + 1..].contains(']') => authority[..i].to_string(),
        _ => authority.to_string(),
    }
}

pub fn is_allowed_url(url: &str) -> bool {
    let lower = url.trim().to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) || lower.contains(char::is_whitespace) {
        return false;
    }
    !is_internal_host(&host_of(&lower))
}

pub struct Page {
    pub url: String,
    pub title: Option<String>,
    pub text: String,
    pub bytes: usize,
}

const MAX_REDIRECTS: usize = 3;
const REDIRECT_MARK: &str = "\n__bilro_redirect__%{redirect_url}";

fn redirect_target(body: &str) -> Option<String> {
    let at = body.rfind("__bilro_redirect__")?;
    let value = body[at + "__bilro_redirect__".len()..].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn strip_redirect_mark(body: &str) -> String {
    match body.rfind("\n__bilro_redirect__") {
        Some(at) => body[..at].to_string(),
        None => body.to_string(),
    }
}

/// Fetches a page and returns its readable text. curl does the transport, which
/// keeps a TLS stack and a certificate store out of this binary. Redirects are
/// followed here rather than by curl, because a page that is allowed may point
/// at one that is not — and `-L` would have gone there without asking.
pub fn fetch(url: &str) -> Result<Page, String> {
    if !is_allowed_url(url) {
        return Err(format!("only http and https are accepted: {url}"));
    }
    let mut target = url.to_string();
    let mut out;
    let mut hops = 0;
    loop {
        out = Command::new("curl")
            .args([
                "-sS",
                "--proto",
                "=http,https",
                "--max-redirs",
                "0",
                "-w",
                REDIRECT_MARK,
                "--max-time",
                &TIMEOUT_SECS.to_string(),
                "--max-filesize",
                &MAX_BYTES.to_string(),
                "-A",
                "bilro",
                &target,
            ])
            .output()
            .map_err(|e| format!("curl unavailable: {e}"))?;
        let body = String::from_utf8_lossy(&out.stdout).to_string();
        let Some(next) = redirect_target(&body) else { break };
        hops += 1;
        if hops > MAX_REDIRECTS {
            return Err(format!("too many redirects from {url}"));
        }
        if !is_allowed_url(&next) {
            return Err(format!("{url} redirects to a target that is not allowed: {next}"));
        }
        target = next;
    }
    let trimmed = strip_redirect_mark(&String::from_utf8_lossy(&out.stdout));
    out.stdout = trimmed.into_bytes();

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
        assert!(is_allowed_url("https://docs.rs/regex/latest/regex/"));
    }

    #[test]
    fn a_fetched_page_cannot_send_the_next_fetch_back_at_this_machine() {
        for u in [
            "http://localhost:3000/x",
            "http://127.0.0.1:7777/api/state",
            "http://169.254.169.254/latest/meta-data/iam/security-credentials/",
            "http://[::1]:7777/",
            "http://192.168.1.1/",
            "http://10.0.0.5:8080/admin",
            "http://0.0.0.0:7777/",
            "http://user:pw@127.0.0.1:7777/",
            "http://db.internal/secrets",
        ] {
            assert!(!is_allowed_url(u), "should refuse an internal target: {u}");
        }
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
