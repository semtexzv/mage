//! Bash tool — execute shell commands with streaming output.
//!
//! Always runs serially (never concurrent-safe). Output streams to the
//! TUI line-by-line as the process runs via `ctx.send_text()`.
//!
//! Working directory persists across invocations.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;
use tokio::io::AsyncBufReadExt;

use mage_core::module::Module;
use mage_core::tool::{ToolContext, ToolDef, ToolHandler};
use mage_core::types::ToolResult;

/// Maximum output size in bytes before truncation.
const MAX_OUTPUT_BYTES: usize = 30_000;

pub struct BashModule;

#[async_trait(?Send)]
impl Module for BashModule {
    fn name(&self) -> &str { "bash" }

    fn tools(&self) -> Vec<ToolDef> {
        let cwd = Rc::new(RefCell::new(
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
        ));
        vec![ToolDef {
            schema: llm::Tool {
                name: "Bash".into(),
                description: "Execute a bash command and return its output. \
                    The working directory persists between calls."
                    .into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The shell command to execute"
                        },
                        "timeout": {
                            "type": "integer",
                            "description": "Timeout in milliseconds (default: 120000, max: 600000)"
                        },
                        "description": {
                            "type": "string",
                            "description": "Human-readable description of what this command does"
                        }
                    },
                    "required": ["command"]
                }),
            },
            handler: Rc::new(BashHandler { cwd }),
        }]
    }
}

struct BashHandler {
    cwd: Rc<RefCell<PathBuf>>,
}

#[async_trait(?Send)]
impl ToolHandler for BashHandler {
    async fn execute(&self, args: serde_json::Value, ctx: ToolContext) -> ToolResult {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c.to_owned(),
            _ => return ToolResult::failure("command is required"),
        };

        let timeout_ms = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(120_000)
            .min(600_000);

        let cwd = self.cwd.borrow().clone();

        let sentinel = "__MAGE_CWD__";
        let wrapped = format!(
            "cd {} && {{ {command} ; }} ; __exit=$?; echo ; echo '{sentinel}' ; pwd ; exit $__exit",
            shell_escape(&cwd.display().to_string()),
        );

        let mut child = match tokio::process::Command::new("bash")
            .arg("-c")
            .arg(&wrapped)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::failure(format!("Failed to spawn bash: {e}")),
        };

        // Take ownership of stdout/stderr for streaming.
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let timeout = tokio::time::Duration::from_millis(timeout_ms);
        let cancel = ctx.cancel_token().clone();

        // Stream stdout and stderr concurrently, line by line.
        let mut all_stdout = String::new();
        let mut all_stderr = String::new();

        let stream_result = tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                return ToolResult::failure("Cancelled");
            }

            _ = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                return ToolResult::failure(format!(
                    "Command timed out after {}s", timeout_ms / 1000
                ));
            }

            result = stream_output(stdout, stderr, &ctx, &mut all_stdout, &mut all_stderr) => {
                result
            }
        };

        if let Err(e) = stream_result {
            return ToolResult::failure(format!("IO error: {e}"));
        }

        // Wait for the process to finish.
        let status = match child.wait().await {
            Ok(s) => s,
            Err(e) => return ToolResult::failure(format!("Wait failed: {e}")),
        };

        // Extract final cwd from stdout sentinel.
        let (user_stdout, new_cwd) = extract_cwd(&all_stdout, sentinel);
        if let Some(dir) = new_cwd {
            *self.cwd.borrow_mut() = PathBuf::from(dir);
        }

        let exit_code = status.code().unwrap_or(-1);

        let mut output = String::new();
        if !user_stdout.is_empty() {
            output.push_str(user_stdout);
        }
        if !all_stderr.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str("STDERR:\n");
            output.push_str(&all_stderr);
        }
        if output.is_empty() {
            output.push_str("(no output)");
        }

        // Truncate if too large.
        if output.len() > MAX_OUTPUT_BYTES {
            let cut = output[..MAX_OUTPUT_BYTES]
                .rfind('\n')
                .unwrap_or(MAX_OUTPUT_BYTES);
            output.truncate(cut);
            output.push_str(&format!(
                "\n\n... output truncated ({} bytes total)",
                user_stdout.len() + all_stderr.len()
            ));
        }

        output.push_str(&format!("\n\nExit code: {exit_code}"));

        // Replace tabs for rendering.
        let output = output.replace('\t', "    ");

        if status.success() {
            ToolResult::success(output)
        } else {
            ToolResult::failure(output)
        }
    }

    fn is_concurrent_safe(&self, _args: &serde_json::Value) -> bool {
        false
    }
}

/// Stream stdout and stderr, sending a complete snapshot of visible output
/// on each new line. The TUI replaces its view each time.
async fn stream_output(
    stdout: Option<tokio::process::ChildStdout>,
    stderr: Option<tokio::process::ChildStderr>,
    ctx: &ToolContext,
    all_stdout: &mut String,
    all_stderr: &mut String,
) -> std::io::Result<()> {
    let stdout_task = async {
        if let Some(out) = stdout {
            let mut reader = tokio::io::BufReader::new(out).lines();
            while let Some(line) = reader.next_line().await? {
                all_stdout.push_str(&line);
                all_stdout.push('\n');

                // Send the complete current view: last N lines of stdout.
                let view = tail_lines(all_stdout, 8).replace('\t', "    ");
                ctx.send_text(view);
            }
        }
        Ok::<_, std::io::Error>(())
    };

    let stderr_task = async {
        if let Some(err) = stderr {
            let mut reader = tokio::io::BufReader::new(err).lines();
            while let Some(line) = reader.next_line().await? {
                all_stderr.push_str(&line);
                all_stderr.push('\n');
            }
        }
        Ok::<_, std::io::Error>(())
    };

    let (r1, r2) = tokio::join!(stdout_task, stderr_task);
    r1?;
    r2?;
    Ok(())
}

/// Return the last `n` lines of text. If more lines exist, prepend "… N more lines".
fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= n {
        return lines.join("\n");
    }
    let skip = lines.len() - n;
    format!("… {} more lines\n{}", skip, lines[skip..].join("\n"))
}

fn extract_cwd<'a>(stdout: &'a str, sentinel: &str) -> (&'a str, Option<&'a str>) {
    if let Some(pos) = stdout.rfind(sentinel) {
        let user_output = stdout[..pos].trim_end();
        let after = stdout[pos + sentinel.len()..].trim();
        let cwd = if after.is_empty() { None } else { Some(after) };
        (user_output, cwd)
    } else {
        (stdout, None)
    }
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
