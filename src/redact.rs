use once_cell::sync::Lazy;
use regex::Regex;

pub const MASK: &str = "***REDACTED***";

static URL_CREDENTIAL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([a-z][a-z0-9+.-]*://)([^\s:@/]*):([^\s@/]+)@").unwrap());

static QUERY_SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)([?&](?:api[_-]?key|access[_-]?token|auth|token|secret|password|passwd|sig|signature|key)=)([^&\s\x22']+)").unwrap()
});

static BEARER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\b((?i:bearer|basic|token))\s+([A-Za-z0-9._~+/=-]*[0-9_.=/+-][A-Za-z0-9._~+/=-]*|[A-Za-z][a-z0-9._~+/=-]*[A-Z][A-Za-z0-9._~+/=-]*)").unwrap());

static JWT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\beyJ[A-Za-z0-9_-]{6,}\.eyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]+").unwrap());

static FLAG_SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(--?(?:password|passwd|token|api[_-]?key|secret|auth)[= ])([^\s\x22']+)").unwrap()
});

static PEM: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)(-----BEGIN [A-Z ]*PRIVATE KEY-----).*?(-----END [A-Z ]*PRIVATE KEY-----)").unwrap()
});

/// A credential carries a digit or a separator; an English word does not. That
/// is what tells `Bearer eyJ0eXAi...` apart from `Bearer authentication`.
/// Length is not the test: it let a five-character token through while masking
/// an ordinary fourteen-letter word. Base64 with no digits is caught by its
/// other tell — a capital letter somewhere other than the first position, which
/// a written word does not have.
///
/// Removes credentials from text on its way to the context window. The name of
/// a field says nothing about whether it holds a secret — a connection string,
/// a query parameter and an Authorization header all carry one under an
/// innocent label — so the value's own shape is what is inspected here.
pub fn redact(text: &str) -> String {
    let out = URL_CREDENTIAL.replace_all(text, |c: &regex::Captures| {
        format!("{}{}:{}@", &c[1], &c[2], MASK)
    });
    let out = QUERY_SECRET.replace_all(&out, |c: &regex::Captures| format!("{}{}", &c[1], MASK));
    let out = JWT.replace_all(&out, MASK);
    let out = BEARER.replace_all(&out, |c: &regex::Captures| format!("{} {}", &c[1], MASK));
    let out = FLAG_SECRET.replace_all(&out, |c: &regex::Captures| format!("{}{}", &c[1], MASK));
    let out = PEM.replace_all(&out, |c: &regex::Captures| format!("{}{}{}", &c[1], MASK, &c[2]));
    out.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaked(output: &str, secret: &str) -> bool {
        output.contains(secret)
    }

    #[test]
    fn secret_in_query_string_does_not_pass() {
        let out = redact("GET https://api.example.com/v1/orders?apikey=SECRETabc123&page=2");
        assert!(!leaked(&out, "SECRETabc123"));
        assert!(out.contains("page=2"), "what is not a secret should stay readable");
        assert!(out.contains("api.example.com"));
    }

    #[test]
    fn authorization_header_does_not_pass() {
        let out = redact("authorization: Bearer abcDEF123456ghiJKL\nx-api: Basic dXNlcjpwYXNz");
        assert!(!leaked(&out, "abcDEF123456ghiJKL"));
        assert!(!leaked(&out, "dXNlcjpwYXNz"));
        assert!(out.contains("Bearer"), "the format stays visible, only the value disappears");
    }

    #[test]
    fn bare_jwt_in_the_middle_of_text_does_not_pass() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let out = redact(&format!("session cookie: {jwt} (expires in 1h)"));
        assert!(!leaked(&out, jwt));
        assert!(out.contains("expires in 1h"));
    }

    #[test]
    fn password_on_command_line_does_not_pass() {
        let out = redact("psql --password=S3cr3tPa55 --host=db.local\nmysql -u root --password S3cr3tOther");
        assert!(!leaked(&out, "S3cr3tPa55"));
        assert!(!leaked(&out, "S3cr3tOther"));
        assert!(out.contains("db.local"));
    }

    #[test]
    fn private_key_pem_does_not_pass() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA7Xk\nsomeLongSecretHere\n-----END RSA PRIVATE KEY-----";
        let out = redact(pem);
        assert!(!leaked(&out, "someLongSecretHere"));
        assert!(out.contains("BEGIN RSA PRIVATE KEY"));
    }

    #[test]
    fn connection_string_stays_covered() {
        let out = redact("DATABASE_URL=postgres://user:s3cr3t@db:5432/prod\nREDIS=redis://:pw123456@cache:6379");
        assert!(!leaked(&out, "s3cr3t"));
        assert!(!leaked(&out, "pw123456"));
        assert!(out.contains("db:5432/prod"));
    }

    #[test]
    fn ordinary_text_passes_through_untouched() {
        for benign in [
            "web-1 1/1 Running 0 5d",
            "compiled in 150ms",
            "https://example.com/docs?page=2&sort=desc",
            "git log --oneline -40",
            "invalid foreign key on the orders table",
        ] {
            assert_eq!(redact(benign), benign, "altered benign text: {benign}");
        }
    }
}

#[cfg(test)]
mod bearer_tests {
    use super::*;

    #[test]
    fn a_short_token_is_still_a_token() {
        for secret in ["LEAK2", "a1b2", "x_9", "pw.1"] {
            let out = redact(&format!("Authorization: Bearer {secret}"));
            assert!(!out.contains(secret), "leaked short token {secret}: {out}");
        }
    }

    #[test]
    fn prose_after_the_word_bearer_is_left_alone() {
        for phrase in [
            "Bearer authentication is required",
            "the token expires hourly",
            "use Basic auth here",
        ] {
            assert_eq!(redact(phrase), phrase, "masked ordinary prose: {phrase}");
        }
    }
}
