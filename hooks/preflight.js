#!/usr/bin/env node
import { readFileSync, writeFileSync, mkdirSync, existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { homedir } from "node:os";
import { fileURLToPath } from "node:url";

import { weighAgents, tokensOf } from "../lib/weigh.js";
import { read as readLedger, write as writeLedger } from "../lib/ledger.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const STATE_DIR = join(homedir(), ".claude", "bilro", "sessions");

const FLOOR_TOKENS = 18000;


function words(text) {
  return new Set(
    String(text || "")
      .toLowerCase()
      .normalize("NFD")
      .replace(/\p{Diacritic}/gu, "")
      .split(/[^a-z0-9]+/)
      .filter((w) => w.length > 3),
  );
}

function overlap(a, b) {
  if (!a.size || !b.size) return 0;
  let hits = 0;
  for (const w of a) if (b.has(w)) hits++;
  return hits / Math.min(a.size, b.size);
}

function agentCost(type, cwd) {
  const dirs = [join(homedir(), ".claude", "agents"), join(cwd, ".claude", "agents")];
  const all = weighAgents(dirs);
  const catalogue = all.reduce((n, a) => n + a.catalogueTokens, 0);
  const hit = all.find((a) => a.name === type);
  return {
    floor: FLOOR_TOKENS + catalogue,
    inheritsEverything: hit ? hit.inheritsEverything : true,
    known: Boolean(hit),
  };
}

let raw = "";
process.stdin.on("data", (c) => (raw += c));
process.stdin.on("end", () => {
  let data = {};
  try {
    data = JSON.parse(raw);
  } catch {
    process.exit(0);
  }

  const tool = data.tool_name || "";
  if (tool !== "Task" && tool !== "Agent") process.exit(0);

  const input = data.tool_input || {};
  const type = input.subagent_type || "general-purpose";
  const description = input.description || "";
  const prompt = input.prompt || "";
  const cwd = data.cwd || process.cwd();

  const state = readLedger(data.session_id);
  const cost = agentCost(type, cwd);
  const promptTokens = tokensOf(prompt);

  const subject = words(`${description} ${prompt}`.slice(0, 600));
  const near = state.dispatches.filter((d) => overlap(subject, new Set(d.words)) > 0.45);

  const lines = [];
  lines.push(
    `bilro: ~${(cost.floor / 1000).toFixed(0)}k de custo fixo + ${promptTokens} tok deste prompt, antes de qualquer trabalho.`,
  );
  if (cost.inheritsEverything) {
    lines.push(
      `  ${type} nao declara tools: — herda o catalogo inteiro de ferramentas neste despacho.`,
    );
  }
  if (near.length) {
    lines.push(
      `  ${near.length}o agente sobre o mesmo assunto nesta sessao (${near.map((d) => d.type).join(", ")}). Da pra medir com ctx_execute?`,
    );
  }
  if (state.dispatches.length >= 8) {
    lines.push(`  ${state.dispatches.length} agentes ja despachados nesta sessao.`);
  }

  state.dispatches.push({
    type,
    words: [...subject].slice(0, 40),
    cost: cost.floor + promptTokens,
    repeat: near.length > 0,
    at: Date.now(),
  });
  writeLedger(data.session_id, state);

  console.log(lines.join("\n"));
  process.exit(0);
});
