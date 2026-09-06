#!/usr/bin/env node
import { open, observe } from "../lib/learn.js";

const MAX_BYTES = 512 * 1024;

let raw = "";
process.stdin.on("data", (c) => (raw += c));
process.stdin.on("end", () => {
  let data = {};
  try {
    data = JSON.parse(raw);
  } catch {
    process.exit(0);
  }
  if (data.tool_name !== "Bash") process.exit(0);

  const command = (data.tool_input || {}).command || "";
  const res = data.tool_response;
  const output = typeof res === "string" ? res : String((res || {}).stdout ?? (res || {}).output ?? "");
  if (!command || !output.trim()) process.exit(0);

  try {
    observe(open(), command, output.length > MAX_BYTES ? output.slice(0, MAX_BYTES) : output);
  } catch (e) { if (process.env.BILRO_DEBUG) console.error(e); }
  process.exit(0);
});
