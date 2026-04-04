//! Bash tool — execute shell commands.
//!
//! Always runs serially (never concurrent-safe) since shell commands
//! can have arbitrary side effects.
//!
//! Working directory persists across invocations via shared state
//! in the module.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

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
        let cwd = Rc::new(RefCell::new(std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))));
        vec![ToolDef {
            schema: llm::Tool {
                name: "Bash".into(),
                description: "Execute a bash command and return its output. \
                    The working directory persists between calls.".into(),
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
    /// Shared working directory state. Updated after each command that
    /// includes `cd`. Only accessed by this handler (serial execution).
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

        // Wrap the command: cd to tracked cwd, run command, then print final cwd.
        // The sentinel lets us extract the final cwd after execution.
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

        let timeout = tokio::time::Duration::from_millis(timeout_ms);
        let cancel = ctx.cancel_token().clone();

        let status = tokio::select! {
            biased;

            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                return ToolResult::failure("Cancelled");
            }

            _ = tokio::time::sleep(timeout) => {
                let _ = child.kill().await;
                return ToolResult::failure(format!(
                    "Command timed out after {}s",
                    timeout_ms / 1000
                ));
            }

            result = child.wait() => {
                match result {
                    Ok(s) => s,
                    Err(e) => return ToolResult::failure(format!("Wait failed: {e}")),
                }
            }
        };

        let stdout = read_pipe(child.stdout.take()).await;
        let stderr = read_pipe(child.stderr.take()).await;

        // Extract final cwd from stdout sentinel.
        let (user_stdout, new_cwd) = extract_cwd(&stdout, sentinel);
        if let Some(dir) = new_cwd {
            *self.cwd.borrow_mut() = PathBuf::from(dir);
        }

        let exit_code = status.code().unwrap_or(-1);

        let mut output = String::new();
        if !user_stdout.is_empty() {
            output.push_str(user_stdout);
        }
        if !stderr.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str("STDERR:\n");
            output.push_str(&stderr);
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
                user_stdout.len() + stderr.len()
            ));
        }

        output.push_str(&format!("\n\nExit code: {exit_code}"));

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

async fn read_pipe<R: tokio::io::AsyncRead + Unpin>(pipe: Option<R>) -> String {
    match pipe {
        Some(mut r) => {
            let mut buf = Vec::new();
            let _ = tokio::io::AsyncReadExt::read_to_end(&mut r, &mut buf).await;
            String::from_utf8_lossy(&buf).into_owned()
        }
        None => String::new(),
    }
}

/// Extract the user's output and the final cwd from stdout.
/// Returns (user_output, Some(cwd)) or (full_stdout, None) if sentinel not found.
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
