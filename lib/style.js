import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";

export const LEVELS = ["off", "lean", "terse"];

const CONFIG = join(homedir(), ".claude", "bilro", "style");

export function readLevel() {
  try {
    const v = readFileSync(CONFIG, "utf8").trim();
    return LEVELS.includes(v) ? v : "off";
  } catch {
    return "off";
  }
}

export function writeLevel(level) {
  if (!LEVELS.includes(level)) throw new Error(`nivel invalido: ${level}`);
  mkdirSync(join(homedir(), ".claude", "bilro"), { recursive: true });
  writeFileSync(CONFIG, level);
  return level;
}

const LEAN = `Escreva enxuto. Corte saudação, preâmbulo, "vou fazer X" antes de fazer, e o resumo do que acabou de ser lido. Uma frase por ideia. Tabela só quando compara três coisas ou mais.`;

const TERSE = `${LEAN}
Corte também: artigo onde a frase sobrevive sem ele, advérbio de intensidade, hedge ("talvez", "acho que") quando você mediu, e a repetição do que o usuário acabou de dizer. Fragmento é aceitável. Termo técnico e mensagem de erro ficam literais.`;

const ALWAYS = `Escreva normal (sem cortes) em: código, mensagem de commit, corpo de PR, aviso de segurança, confirmação de ação irreversível, e passo a passo onde a ordem importa.`;

const ANTIDRIFT = `Esta regra vale para TODA resposta desta sessão, inclusive relatório de status e resultado de comando. Instrução dita uma vez decai em conversa longa — se estiver em dúvida, corte.`;

/** The rules re-emitted every session, since a rule stated once decays. */
export function ruleset(level = readLevel()) {
  if (level === "off") return "";
  const body = level === "terse" ? TERSE : LEAN;
  return [`bilro style: ${level}`, body, ALWAYS, ANTIDRIFT].join("\n");
}
