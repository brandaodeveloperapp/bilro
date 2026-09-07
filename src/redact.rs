use once_cell::sync::Lazy;
use regex::Regex;

pub const MASK: &str = "***MASCARADO***";

static URL_CREDENTIAL: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([a-z][a-z0-9+.-]*://)([^\s:@/]*):([^\s@/]+)@").unwrap());

static QUERY_SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)([?&](?:api[_-]?key|access[_-]?token|auth|token|secret|password|passwd|senha|sig|signature|key)=)([^&\s\x22']+)").unwrap()
});

static BEARER: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(bearer|basic|token)\s+([A-Za-z0-9._~+/=-]{12,})").unwrap());

static JWT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\beyJ[A-Za-z0-9_-]{6,}\.eyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]+").unwrap());

static FLAG_SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(--?(?:password|passwd|senha|token|api[_-]?key|secret|auth)[= ])([^\s\x22']+)").unwrap()
});

static PEM: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?s)(-----BEGIN [A-Z ]*PRIVATE KEY-----).*?(-----END [A-Z ]*PRIVATE KEY-----)").unwrap()
});

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

    fn vazou(saida: &str, segredo: &str) -> bool {
        saida.contains(segredo)
    }

    #[test]
    fn segredo_em_query_string_nao_passa() {
        let out = redact("GET https://api.exemplo.com/v1/pedidos?apikey=SEGREDOabc123&page=2");
        assert!(!vazou(&out, "SEGREDOabc123"));
        assert!(out.contains("page=2"), "o que nao e segredo continua legivel");
        assert!(out.contains("api.exemplo.com"));
    }

    #[test]
    fn cabecalho_de_autorizacao_nao_passa() {
        let out = redact("authorization: Bearer abcDEF123456ghiJKL\nx-api: Basic dXNlcjpzZW5oYQ==");
        assert!(!vazou(&out, "abcDEF123456ghiJKL"));
        assert!(!vazou(&out, "dXNlcjpzZW5oYQ=="));
        assert!(out.contains("Bearer"), "o formato continua visivel, so o valor some");
    }

    #[test]
    fn jwt_solto_no_meio_do_texto_nao_passa() {
        let jwt = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let out = redact(&format!("cookie de sessao: {jwt} (expira em 1h)"));
        assert!(!vazou(&out, jwt));
        assert!(out.contains("expira em 1h"));
    }

    #[test]
    fn senha_em_linha_de_comando_nao_passa() {
        let out = redact("psql --password=S3nh4Sup3r --host=db.local\nmysql -u root --senha S3nh4Outra");
        assert!(!vazou(&out, "S3nh4Sup3r"));
        assert!(!vazou(&out, "S3nh4Outra"));
        assert!(out.contains("db.local"));
    }

    #[test]
    fn chave_privada_pem_nao_passa() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA7Xk\nsegredoLongoAqui\n-----END RSA PRIVATE KEY-----";
        let out = redact(pem);
        assert!(!vazou(&out, "segredoLongoAqui"));
        assert!(out.contains("BEGIN RSA PRIVATE KEY"));
    }

    #[test]
    fn string_de_conexao_continua_coberta() {
        let out = redact("DATABASE_URL=postgres://user:s3nh4@db:5432/prod\nREDIS=redis://:pw123456@cache:6379");
        assert!(!vazou(&out, "s3nh4"));
        assert!(!vazou(&out, "pw123456"));
        assert!(out.contains("db:5432/prod"));
    }

    #[test]
    fn texto_comum_passa_intacto() {
        for benigno in [
            "web-1 1/1 Running 0 5d",
            "compilou em 150ms",
            "https://exemplo.com/docs?page=2&sort=desc",
            "git log --oneline -40",
            "chave estrangeira invalida na tabela pedidos",
        ] {
            assert_eq!(redact(benigno), benigno, "alterou texto benigno: {benigno}");
        }
    }
}
