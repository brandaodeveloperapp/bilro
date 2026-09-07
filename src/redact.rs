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

static NAMED_SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(concat!(
        r#"(?im)((?:^|[\s{,\[])["']?[A-Za-z0-9_.-]*?"#,
        r#"(?:SERVICE[_-]?ROLE[_-]?KEY|PRIVATE[_-]?KEY|ACCESS[_-]?KEY|SECRET[_-]?KEY"#,
        r#"|API[_-]?KEY|APIKEY|AUTHORIZATION|PASSWORD|PASSWD|CREDENTIALS?|_?AUTH[_-]?TOKEN"#,
        r#"|KEY[_-]?DATA|CERTIFICATE[_-]?DATA|SET[_-]?COOKIE|COOKIE|TOKEN|SECRET|AUTH|DSN)S?"#,
        r#"["']?\s*[:=]\s*["']?)(\$\{[^}]*\}|[^\s"',{}\]]+)"#,
    ))
    .unwrap()
});

/// A crypt hash names itself: `$2y$` is bcrypt, `$6$` is sha512-crypt. The
/// field it sits under is often called something harmless like `hash`.
static CRYPT_HASH: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"\$(?:2[abxy]?|1|5|6|apr1|y|argon2[a-z]*)\$[^\s"',]+"#).unwrap());

static AWS_KEY_ID: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").unwrap());

/// `.pgpass` and `.netrc` carry a credential in a position, not under a name.
/// The `.netrc` rule is anchored to a whole line because the loose form ate the
/// word after `password` in ordinary prose, turning `password authentication
/// failed` — the one line worth reading — into nonsense.
static PGPASS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?m)^([^:\s]+:\d+:[^:]*:[^:]*:)(\S+)$").unwrap());
static NETRC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?im)^(\s*(?:machine\s+\S+\s+)?(?:login\s+\S+\s+)?password\s+)(\S+)\s*$").unwrap()
});

/// `$MY_VAR` and `${MY_VAR}` are references to a secret, not the secret. But
/// `$uperSecret123` and a bcrypt hash `$2y$10$...` both start with a dollar and
/// are the real thing, so the dollar alone decides nothing.
fn is_a_variable_reference(value: &str) -> bool {
    let Some(rest) = value.strip_prefix('$') else { return false };
    match rest.chars().next() {
        Some('{') => true,
        Some(c) if c.is_ascii_uppercase() || c == '_' => {
            rest.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        }
        _ => false,
    }
}

/// `MAX_TOKENS=1500` is a budget, not a credential, and masking it breaks the
/// output for no gain. A secret is never a bare number or a flag word — nor the
/// scheme word of an Authorization header, whose own value an earlier rule has
/// already masked.
fn looks_like_a_setting(value: &str) -> bool {
    if value.starts_with(MASK) {
        return true;
    }
    if matches!(
        value.to_ascii_lowercase().as_str(),
        "bearer" | "basic" | "digest" | "negotiate" | "token"
    ) {
        return true;
    }
    if value.starts_with('/')
        || value.starts_with('<')
        || value.starts_with('*')
        || value.starts_with("process.env")
        || is_a_variable_reference(value)
    {
        return true;
    }
    value.parse::<f64>().is_ok()
        || matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "none" | "nil" | "yes" | "no" | "on" | "off" | "strict" | "required" | "optional" | "disabled" | "enabled"
        )
}

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
/// innocent label — so the value's own shape is what is inspected here. The one
/// place the label does decide is `NAME=value`, where the name is the only
/// evidence there is: `DB_PASSWORD=hunter2` looks like any other assignment.
pub fn redact(text: &str) -> String {
    let out = URL_CREDENTIAL.replace_all(text, |c: &regex::Captures| {
        format!("{}{}:{}@", &c[1], &c[2], MASK)
    });
    let out = QUERY_SECRET.replace_all(&out, |c: &regex::Captures| format!("{}{}", &c[1], MASK));
    let out = JWT.replace_all(&out, MASK);
    let out = BEARER.replace_all(&out, |c: &regex::Captures| format!("{} {}", &c[1], MASK));
    let out = FLAG_SECRET.replace_all(&out, |c: &regex::Captures| format!("{}{}", &c[1], MASK));
    let out = PEM.replace_all(&out, |c: &regex::Captures| format!("{}{}{}", &c[1], MASK, &c[2]));
    let out = NAMED_SECRET.replace_all(&out, |c: &regex::Captures| {
        if looks_like_a_setting(&c[2]) {
            c[0].to_string()
        } else {
            format!("{}{}", &c[1], MASK)
        }
    });
    let out = AWS_KEY_ID.replace_all(&out, MASK);
    let out = CRYPT_HASH.replace_all(&out, MASK);
    let out = PGPASS.replace_all(&out, |c: &regex::Captures| format!("{}{}", &c[1], MASK));
    let out = NETRC.replace_all(&out, |c: &regex::Captures| format!("{}{}", &c[1], MASK));
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

#[cfg(test)]
mod named_secret_tests {
    use super::*;

    fn leaked(output: &str, secret: &str) -> bool {
        output.contains(secret)
    }

    #[test]
    fn a_dollar_does_not_make_a_value_a_variable() {
        for (line, secret) in [
            ("DB_PASSWORD=$uperFAKEsecret123", "$uperFAKEsecret123"),
            ("TOKEN=$FAKEliteralXYZ789", "$FAKEliteralXYZ789"),
            ("HASH_SECRET=$2y$10$FAKEbcrypthashvalue", "FAKEbcrypthashvalue"),
            ("hash=$2y$10$FAKEbcrypthashvalue", "FAKEbcrypthashvalue"),
            ("igor:$6$FAKEsha512cryptvalue:19000:0:99999:7:::", "FAKEsha512cryptvalue"),
            ("API_KEY=$1$FAKEmd5crypt", "FAKEmd5crypt"),
        ] {
            let out = redact(line);
            assert!(!leaked(&out, secret), "leaked from {line}: {out}");
        }
    }

    #[test]
    fn a_real_variable_reference_is_left_readable() {
        for line in ["API_KEY=$MY_VAR", "TOKEN=${SECRET_TOKEN}", "PASSWORD=$DB_PW_2"] {
            assert_eq!(redact(line), line, "masked a variable reference: {line}");
        }
    }

    #[test]
    fn the_word_password_in_prose_is_not_a_credential() {
        for line in [
            "FATAL: password authentication failed for user \"app\"",
            "the password field is required",
            "password mismatch for user bob",
            "reset your password now",
        ] {
            assert_eq!(redact(line), line, "ate a word out of a diagnostic: {line}");
        }
    }

    #[test]
    fn a_real_netrc_line_still_disappears() {
        let out = redact("machine api.example.com login igor password FAKEnetrc02\n");
        assert!(!leaked(&out, "FAKEnetrc02"), "{out}");
        assert!(out.contains("igor"), "{out}");
    }

    #[test]
    fn the_shells_own_pwd_is_not_a_secret() {
        assert_eq!(redact("PWD=relative/path/here"), "PWD=relative/path/here");
        assert_eq!(redact("PWD=/Users/igor/projeto"), "PWD=/Users/igor/projeto");
    }

    #[test]
    fn the_shapes_a_secret_really_arrives_in() {
        let cases = [
            ("PASSWORD=FAKEpw123", "FAKEpw123"),
            ("TOKEN=FAKEtok456", "FAKEtok456"),
            ("SECRET_KEY=FAKEdjango789", "FAKEdjango789"),
            ("APIKEY=FAKEapi001", "FAKEapi001"),
            ("PASSWD=FAKEpwd002", "FAKEpwd002"),
            ("OPENAI_API_KEY=FAKEoai003", "FAKEoai003"),
            ("SUPABASE_SERVICE_ROLE_KEY=FAKEsb004", "FAKEsb004"),
            ("  \"password\": \"FAKEjson111\"", "FAKEjson111"),
            ("  \"SecretAccessKey\": \"FAKEaws112\"", "FAKEaws112"),
            ("password: FAKEyaml222", "FAKEyaml222"),
            ("x-api-key: FAKEhdr555", "FAKEhdr555"),
            ("authorization: FAKEauth333", "FAKEauth333"),
            ("SENTRY_DSN=https://FAKEdsn444@o1.ingest.sentry.io/2", "FAKEdsn444"),
            ("AWS_KEY=AKIAIOSFODNN7EXAMPLE", "AKIAIOSFODNN7EXAMPLE"),
            ("aws_access_key_id = ASIAIOSFODNN7EXAMPLE", "ASIAIOSFODNN7EXAMPLE"),
            ("export GITHUB_TOKEN=FAKEgh006", "FAKEgh006"),
        ];
        for (line, secret) in cases {
            let out = redact(line);
            assert!(!leaked(&out, secret), "leaked from {line:?}: {out}");
        }
    }

    #[test]
    fn a_setting_that_merely_sounds_like_a_secret_survives_intact() {
        let kept = [
            "MAX_TOKENS=1500",
            "TEST_PASS=14",
            "X_PASS_RATE=0.97",
            "CLAUDE_TOKEN_BUDGET=200000",
            "DB_PASSWORD_POLICY=strict",
            "JWT_AUTH_MODE=oauth",
            "AWS_ACCESS_KEY_ID_FILE=/etc/x",
            "RETRY_COUNT=3",
            "NODE_ENV=production",
            "PORT=8080",
        ];
        for line in kept {
            let out = redact(line);
            assert_eq!(out, line, "redacted a plain setting: {line}");
        }
    }

    #[test]
    fn the_authorization_scheme_word_is_not_the_secret() {
        let out = redact("authorization: Bearer abcDEF123456ghiJKL");
        assert!(out.contains("Bearer"), "{out}");
        assert!(!leaked(&out, "abcDEF123456ghiJKL"), "{out}");
    }

    #[test]
    fn an_env_file_does_not_reach_the_context_window() {
        let env = "DATABASE_URL=postgres://app:pw123@db:5432/prod\n\
                   EVOLUTION_API_KEY=sk-live-9f8a7b6c5d4e\n\
                   DB_PASSWORD=Sup3rS3cr3t\n\
                   AUTHENTICATION_API_KEY=B6D711FCDE4D4FD5936544120E713976\n\
                   export STRIPE_SECRET_KEY=sk_test_abc123\n\
                   aws_access_key_id: AKIAIOSFODNN7EXAMPLE\n";
        let out = redact(env);
        for secret in [
            "sk-live-9f8a7b6c5d4e",
            "Sup3rS3cr3t",
            "B6D711FCDE4D4FD5936544120E713976",
            "sk_test_abc123",
            "AKIAIOSFODNN7EXAMPLE",
        ] {
            assert!(!leaked(&out, secret), "leaked {secret} in:\n{out}");
        }
    }

    #[test]
    fn an_ordinary_assignment_is_left_alone() {
        let out = redact("PORT=8080\nNODE_ENV=production\nUSER_NAME=igor\nRETRY_COUNT=3\n");
        assert!(out.contains("PORT=8080"));
        assert!(out.contains("NODE_ENV=production"));
        assert!(out.contains("USER_NAME=igor"));
        assert!(!out.contains(MASK));
    }
}
