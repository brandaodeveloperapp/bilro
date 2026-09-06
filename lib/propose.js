import { open, signature } from "./learn.js";

const FAILURE = /\b(error|erro|failed|falhou|fatal|exception|traceback|refused|denied|not found|cannot|timeout)\b/i;

/**
 * A command shape run many times is a recipe someone had to rediscover; a
 * failure line seen across several runs is a trap that will be stepped on
 * again. Both are memories waiting to be written.
 */
export function proposals(db = open(), { minRuns = 5, minFailureRuns = 2 } = {}) {
  const out = [];

  for (const row of db.prepare("SELECT sig, n FROM runs WHERE n >= ? ORDER BY n DESC LIMIT 12").all(minRuns)) {
    out.push({
      kind: "receita",
      subject: row.sig,
      seen: row.n,
      why: `rodado ${row.n} vezes — vale virar receita com \`verify:\``,
    });
  }

  const lines = db
    .prepare(
      `SELECT l.sig, l.df, e.h, x.body FROM lines l
       JOIN exact e ON e.sig = l.sig AND e.shape = l.h
       LEFT JOIN last x ON x.sig = l.sig
       WHERE l.df >= ? ORDER BY l.df DESC LIMIT 200`,
    )
    .all(minFailureRuns);

  const seen = new Set();
  for (const row of lines) {
    if (!row.body) continue;
    const hit = row.body.split("\n").find((l) => FAILURE.test(l) && l.trim().length > 15);
    if (!hit || seen.has(hit)) continue;
    seen.add(hit);
    out.push({
      kind: "armadilha",
      subject: hit.trim().slice(0, 120),
      seen: row.df,
      why: `apareceu em ${row.df} execucoes de \`${row.sig.slice(0, 40)}\``,
    });
    if (out.filter((o) => o.kind === "armadilha").length >= 6) break;
  }

  return out;
}

/** A memory file the owner only has to edit, not write from nothing. */
export function draft(proposal) {
  const slug = proposal.subject
    .toLowerCase()
    .normalize("NFD")
    .replace(/\p{Diacritic}/gu, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 48);
  const body =
    proposal.kind === "receita"
      ? `Comando rodado ${proposal.seen} vezes:\n\n    ${proposal.subject}\n\nEscreva aqui o que ele resolve e o que quebra quando falha.`
      : `Falha vista em ${proposal.seen} execucoes:\n\n    ${proposal.subject}\n\nEscreva aqui a causa e o conserto, para nao redescobrir.`;
  return {
    slug,
    content: `---\nname: ${slug}\ndescription: ${proposal.why}\nmetadata:\n  type: project\n---\n\n${body}\n`,
  };
}
