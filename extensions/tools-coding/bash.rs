//! Bash tool — executes shell commands and returns stdout/stderr.
//!
//! Registers a closure-based tool that spawns the command via `tokio::process`,
//! captures combined stdout+stderr, and returns the output as a `ToolResult`.
//!
//! Respects cancellation — if the agent loop cancels, the child process
//! is killed.

// @dep tokio = { version = "1", features = ["process", "io-util"] }
// @dep serde_json = "1"

use mage::prelude::*;
use serde_json::json;

/// JSON schema for the bash tool's parameters.
fn bash_params() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "The shell command to execute"
            },
            "timeout": {
                "type": "integer",
                "description": "Timeout in seconds (default: 120)"
            }
        },
        "required": ["command"]
    })
}

struct BashExt;

#[async_trait(?Send)]
impl Extension for BashExt {
    fn init(&mut self, reg: &mut ExtensionRegistry) {
        reg.tool(
            llm::Tool {
                name: "bash".into(),
                description: "Execute a bash command and return its output. Use for running \
                    shell commands, installing packages, running tests, and general system \
                    operations.".into(),
                parameters: bash_params(),
            },
            |_call_id, params, handle| async move {
                let command = params
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();

                let timeout_secs = params
                    .get("timeout")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(120);

                if command.is_empty() {
                    return ToolResult::failure("command parameter is required");
                }

                let mut child = match tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(&command)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(e) => return ToolResult::failure(format!("failed to spawn bash: {e}")),
                };

                let stdout_handle = child.stdout.take();
                let stderr_handle = child.stderr.take();

                let timeout = tokio::time::Duration::from_secs(timeout_secs);
                let cancel = handle.cancel_token();

                let status = tokio::select! {
                    s = child.wait() => match s {
                        Ok(s) => s,
                        Err(e) => return ToolResult::failure(format!("wait failed: {e}")),
                    },
                    _ = tokio::time::sleep(timeout) => {
                        let _ = child.kill().await;
                        return ToolResult::failure(
                            format!("command timed out after {timeout_secs}s"),
                        );
                    },
                    () = std::future::ready(()), if cancel.is_cancelled() => {
                        let _ = child.kill().await;
                        return ToolResult::failure("cancelled".to_string());
                    },
                };

                // Read captured output.
                let stdout = if let Some(mut h) = stdout_handle {
                    let mut buf = Vec::new();
                    let _ = tokio::io::AsyncReadExt::read_to_end(&mut h, &mut buf).await;
                    String::from_utf8_lossy(&buf).into_owned()
                } else {
                    String::new()
                };
                let stderr = if let Some(mut h) = stderr_handle {
                    let mut buf = Vec::new();
                    let _ = tokio::io::AsyncReadExt::read_to_end(&mut h, &mut buf).await;
                    String::from_utf8_lossy(&buf).into_owned()
                } else {
                    String::new()
                };

                let exit_code = status.code().unwrap_or(-1);

                let mut text = String::new();
                if !stdout.is_empty() {
                    text.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str("STDERR:\n");
                    text.push_str(&stderr);
                }
                if text.is_empty() {
                    text.push_str("(no output)");
                }
                text.push_str(&format!("\n\nExit code: {exit_code}"));
                if status.success() {
                    ToolResult::success(text)
                } else {
                    ToolResult::failure(text)
                }
            },
        );
    }
}

/// Module entry point — returns extensions provided by this module.
pub fn extensions() -> Vec<Box<dyn Extension>> {
    vec![Box::new(BashExt)]
}
