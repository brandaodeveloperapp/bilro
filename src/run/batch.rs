use crate::proc::spawn_with_timeout;
use crate::redact::redact;
use crate::store::sandbox::{index, open_default, search, QueryHit};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const DEFAULT_BATCH_TIMEOUT_MS: u64 = 5_000;
const MAX_CONCURRENCY: usize = 8;

/// One command to run as part of a batch, with a label used as the index
/// chunk title so search results can be traced back to it.
#[derive(Debug, Clone)]
pub struct Command {
    pub label: String,
    pub command: String,
}

/// What happened to a single command in the batch: whether it failed, how
/// much raw output it produced, and how many chunks it left in the index.
#[derive(Debug)]
pub struct CommandOutcome {
    pub label: String,
    pub failed: bool,
    pub timed_out: bool,
    pub raw_bytes: usize,
    pub chunks: usize,
}

/// Result of a batch run: per-command outcomes in input order, plus whatever
/// the given queries turned up in the freshly indexed output.
pub struct BatchResult {
    pub outcomes: Vec<CommandOutcome>,
    pub hits: Vec<QueryHit>,
}

fn run_one(command: &Command, cwd: Option<&Path>, timeout_ms: u64) -> (CommandOutcome, String) {
    let mut sh = ProcessCommand::new("/bin/sh");
    sh.arg("-c").arg(&command.command);
    if let Some(dir) = cwd {
        sh.current_dir(dir);
    }

    let (failed, timed_out, raw) = match spawn_with_timeout(sh, timeout_ms) {
        Ok(o) if o.timed_out => {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&err);
            }
            text.push_str(&format!("\n[bilro: interrupted after {}ms]", timeout_ms));
            (true, true, text)
        }
        Ok(o) => {
            let ok = o.status.map(|s| s.success()).unwrap_or(false);
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            let err = String::from_utf8_lossy(&o.stderr);
            if !ok && !err.trim().is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&err);
            }
            (!ok, false, text)
        }
        Err(e) => (true, false, e.to_string()),
    };

    let outcome = CommandOutcome {
        label: command.label.clone(),
        failed,
        timed_out,
        raw_bytes: raw.len(),
        chunks: 0,
    };
    (outcome, raw)
}

/// Runs `commands` with at most `concurrency` (clamped to 1..=8) in flight at
/// once, indexes every command's redacted output under its label, then runs
/// `queries` against the freshly indexed chunks. Output order in the result
/// matches input order regardless of which command finished first.
pub fn run(commands: &[Command], queries: &[String], concurrency: usize, cwd: Option<&Path>) -> BatchResult {
    let concurrency = concurrency.clamp(1, MAX_CONCURRENCY);
    let cwd_owned = cwd.map(Path::to_path_buf);
    let commands_arc: Arc<Vec<Command>> = Arc::new(commands.to_vec());
    let results: Arc<Mutex<Vec<Option<CommandOutcome>>>> =
        Arc::new(Mutex::new((0..commands.len()).map(|_| None).collect()));
    let next = Arc::new(AtomicUsize::new(0));

    let workers = concurrency.min(commands.len());
    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let next = Arc::clone(&next);
        let results = Arc::clone(&results);
        let commands_arc = Arc::clone(&commands_arc);
        let cwd_owned = cwd_owned.clone();
        handles.push(thread::spawn(move || loop {
            let i = next.fetch_add(1, Ordering::SeqCst);
            if i >= commands_arc.len() {
                break;
            }
            let command = &commands_arc[i];
            let (partial, raw) = run_one(command, cwd_owned.as_deref(), DEFAULT_BATCH_TIMEOUT_MS);
            let sanitized = redact(&raw);
            let chunks = open_default()
                .and_then(|conn| index(&conn, &command.label, &sanitized, "batch"))
                .unwrap_or(0);
            let outcome = CommandOutcome { chunks, ..partial };
            results.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(outcome);
        }));
    }
    for h in handles {
        let _ = h.join();
    }

    let outcomes: Vec<CommandOutcome> = Arc::try_unwrap(results)
        .map(|m| m.into_inner().unwrap_or_default())
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect();

    let mut hits = Vec::new();
    if !queries.is_empty() {
        if let Ok(conn) = open_default() {
            for q in queries {
                if let Ok(found) = search(&conn, q, 5) {
                    hits.extend(found.into_iter().map(|hit| QueryHit { query: q.clone(), hit }));
                }
            }
        }
    }

    BatchResult { outcomes, hits }
}

/// JSON Schema for the `bilro_batch` MCP tool.
pub fn mcp_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "name": "bilro_batch",
        "description": "Runs commands in parallel (bounded concurrency), indexes each one's output under its label, and returns only the chunks matching the queries instead of the entire raw text.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "commands": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": { "type": "string", "description": "chunk title in the index; a descriptive label improves search" },
                            "command": { "type": "string", "description": "shell command to run" }
                        },
                        "required": ["label", "command"]
                    }
                },
                "find": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "terms searched in the index once every command has run"
                },
                "concurrency": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 8,
                    "description": "how many commands in flight at once; default 1"
                },
                "cwd": { "type": "string", "description": "working directory for every command" }
            },
            "required": ["commands"]
        }
    })
}

fn outcome_to_json(o: &CommandOutcome) -> serde_json::Value {
    serde_json::json!({
        "label": o.label,
        "failed": o.failed,
        "timed_out": o.timed_out,
        "raw_bytes": o.raw_bytes,
        "chunks": o.chunks,
    })
}

fn hit_to_json(h: &QueryHit) -> serde_json::Value {
    serde_json::json!({
        "query": h.query,
        "label": h.hit.label,
        "body": h.hit.body,
        "source": h.hit.source,
        "score": h.hit.score,
    })
}

fn parse_commands(value: &serde_json::Value) -> Result<Vec<Command>, String> {
    let raw = value.as_array().ok_or("commands must be a list")?;
    if raw.is_empty() {
        return Err("commands is empty".into());
    }
    raw.iter()
        .map(|c| {
            let label = c.get("label").and_then(|v| v.as_str()).ok_or("command missing label")?.to_string();
            let command = c.get("command").and_then(|v| v.as_str()).ok_or("command missing command")?.to_string();
            if command.trim().is_empty() {
                return Err("command is empty".into());
            }
            Ok(Command { label, command })
        })
        .collect()
}

/// Executes `bilro_batch` from raw MCP tool arguments, returning a JSON
/// string with per-command outcomes and query hits.
pub fn mcp_call(args: &serde_json::Value) -> Result<String, String> {
    let commands_value = args.get("commands").ok_or("commands is required")?;
    let commands = parse_commands(commands_value)?;

    let queries: Vec<String> = args
        .get("find")
        .or_else(|| args.get("queries"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    let concurrency = args.get("concurrency").and_then(serde_json::Value::as_u64).unwrap_or(1) as usize;
    let cwd = args.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from);

    let result = run(&commands, &queries, concurrency, cwd.as_deref());

    let mut out = String::new();
    for o in &result.outcomes {
        out.push_str(&format!(
            "{} {} — {} bytes in {} chunk(s){}\n",
            if o.failed { "failed" } else { "ok" },
            o.label,
            o.raw_bytes,
            o.chunks,
            if o.timed_out { ", interrupted by timeout" } else { "" }
        ));
    }
    if result.hits.is_empty() {
        out.push_str(if queries.is_empty() {
            "\nOutput indexed. Pass find to get the chunks, or use bilro_find afterward."
        } else {
            "\nNo chunk matched what was asked for. The output is indexed; try another term with bilro_find."
        });
    } else {
        for h in &result.hits {
            out.push_str(&format!("\n## {} — {}\n{}\n", h.query, h.hit.label, h.hit.body));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn cmd(label: &str, command: &str) -> Command {
        Command { label: label.to_string(), command: command.to_string() }
    }

    #[test]
    fn four_fast_commands_all_run_in_order() {
        let commands = vec![
            cmd("one", "echo 1"),
            cmd("two", "echo 2"),
            cmd("three", "echo 3"),
            cmd("four", "echo 4"),
        ];
        let result = run(&commands, &[], 4, None);
        assert_eq!(result.outcomes.len(), 4);
        let labels: Vec<&str> = result.outcomes.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, vec!["one", "two", "three", "four"]);
        assert!(result.outcomes.iter().all(|o| !o.failed));
    }

    #[test]
    fn accepts_find_and_the_old_name_queries() {
        for key in ["find", "queries"] {
            let args = serde_json::json!({
                "commands": [{"label": "echo", "command": "echo rare-term-xylophone"}],
                key: ["xylophone"]
            });
            let output = mcp_call(&args).expect("should run");
            assert!(output.contains("xylophone"), "key {key} did not bring the chunk: {output}");
        }
    }

    #[test]
    fn no_matching_chunk_says_what_to_do_instead_of_staying_silent() {
        let args = serde_json::json!({
            "commands": [{"label": "echo", "command": "echo something"}],
            "find": ["term-that-does-not-exist-anywhere"]
        });
        let output = mcp_call(&args).unwrap();
        assert!(output.contains("bilro_find"), "should say how to continue: {output}");
    }

    #[test]
    fn real_parallelism_is_much_faster_than_serial() {
        let commands = vec![
            cmd("a", "sleep 0.4"),
            cmd("b", "sleep 0.4"),
            cmd("c", "sleep 0.4"),
            cmd("d", "sleep 0.4"),
        ];

        let serial_start = Instant::now();
        let serial = run(&commands, &[], 1, None);
        let serial_time = serial_start.elapsed();

        let parallel_start = Instant::now();
        let parallel = run(&commands, &[], 4, None);
        let parallel_time = parallel_start.elapsed();

        assert_eq!(serial.outcomes.len(), 4);
        assert_eq!(parallel.outcomes.len(), 4);
        assert!(
            parallel_time < serial_time / 2,
            "parallel ({:?}) should be much faster than serial ({:?})",
            parallel_time,
            serial_time
        );
    }

    #[test]
    fn huge_output_does_not_hang_and_does_not_lose_a_line() {
        let commands = vec![cmd("huge", "seq 1 200000")];
        let start = Instant::now();
        let result = run(&commands, &[], 1, None);
        assert!(!result.outcomes[0].failed, "should not fail or hang until the timeout");
        assert!(result.outcomes[0].raw_bytes > 1_000_000, "output came back too truncated: {} bytes", result.outcomes[0].raw_bytes);
        assert!(start.elapsed().as_secs() < 15, "took {}s", start.elapsed().as_secs());
    }

    #[test]
    fn one_failing_command_does_not_block_the_others() {
        let commands = vec![cmd("ok-a", "echo a"), cmd("failure", "exit 7"), cmd("ok-b", "echo b")];
        let result = run(&commands, &[], 3, None);
        assert_eq!(result.outcomes.len(), 3);
        assert!(!result.outcomes[0].failed);
        assert!(result.outcomes[1].failed, "should mark the command that failed");
        assert!(!result.outcomes[2].failed);
    }

    #[test]
    fn hung_command_is_interrupted_by_the_timeout() {
        let commands = vec![cmd("hung", "sleep 30")];
        let start = Instant::now();
        let result = run(&commands, &[], 1, None);
        assert!(result.outcomes[0].timed_out, "should have marked a timeout");
        assert!(start.elapsed().as_secs() < 10, "batch did not finish shortly after the command's timeout");
    }

    #[test]
    fn secret_does_not_appear_in_the_index_or_the_result() {
        let commands = vec![cmd("secret", "echo 'DB=postgres://u:S3CR3T@h:5432/d'")];
        let result = run(&commands, &["S3CR3T".to_string()], 1, None);
        assert!(result.hits.is_empty(), "searching for the secret itself should find nothing indexed");

        let conn = open_default().unwrap();
        let found = search(&conn, "postgres", 5).unwrap();
        for h in &found {
            assert!(!h.body.contains("S3CR3T"), "leaked into the index: {}", h.body);
        }
    }

    #[test]
    fn query_returns_the_chunk_of_the_right_command_by_label() {
        let commands = vec![
            cmd("report-alpha", "echo rare-term-alpha-xyz"),
            cmd("report-beta", "echo something-completely-different"),
        ];
        let result = run(&commands, &["rare-term-alpha-xyz".to_string()], 2, None);
        assert!(!result.hits.is_empty(), "should have found the rare term");
        assert!(result.hits.iter().any(|h| h.hit.label == "report-alpha"));
        assert!(result.hits.iter().all(|h| h.hit.label != "report-beta"));
    }

    #[test]
    fn mcp_schema_declares_the_bilro_batch_tool() {
        let schema = mcp_tool_schema();
        assert_eq!(schema["name"], "bilro_batch");
        assert!(schema["inputSchema"]["properties"]["commands"].is_object());
    }

    #[test]
    fn mcp_call_reports_each_command_in_readable_text() {
        let args = serde_json::json!({
            "commands": [
                {"label": "first", "command": "echo one"},
                {"label": "second", "command": "exit 3"}
            ]
        });
        let output = mcp_call(&args).expect("should run");
        assert!(output.contains("first"), "missing the first label: {output}");
        assert!(output.contains("second"), "missing the second label: {output}");
        assert!(output.contains("failed"), "should mark what failed: {output}");
        assert!(output.contains("ok"), "should mark what passed: {output}");
    }

    #[test]
    fn mcp_call_without_commands_is_a_clear_error() {
        let err = mcp_call(&serde_json::json!({})).unwrap_err();
        assert!(err.contains("commands"));
    }
}
