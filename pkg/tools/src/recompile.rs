//! Recompile tool — lets the agent trigger self-recompilation.
//!
//! Uses `MageBuild::generate_workspace()` for the sync prep, then
//! runs `cargo build` as an async tokio subprocess, streaming stderr
//! to the tool widget line by line.

use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;
use tokio::io::AsyncBufReadExt;

use mage_core::module::Module;
use mage_core::tool::{ToolContext, ToolDef, ToolHandler};
use mage_core::types::ToolResult;

pub struct RecompileModule;

#[async_trait(?Send)]
impl Module for RecompileModule {
    fn name(&self) -> &str { "recompile" }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            schema: llm::Tool {
                name: "Recompile".into(),
                description: "Recompile the agent binary with modules from ~/.mage/modules/. \
                    Modules listed in force_local override snapshot modules with the same name. \
                    After compilation under the monitor, the agent restarts with the new binary.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "force_local": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Module names to force from ~/.mage/modules/ (overrides snapshot versions)"
                        }
                    },
                }),
            },
            handler: Rc::new(RecompileHandler),
        }]
    }
}

struct RecompileHandler;

#[async_trait(?Send)]
impl ToolHandler for RecompileHandler {
    async fn execute(&self, args: serde_json::Value, ctx: ToolContext) -> ToolResult {
        let force_local: Vec<String> = args
            .get("force_local")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Log callback: accumulates lines, sends last 8 as the tool view.
        let log_lines = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let update_tx = ctx.update_sender();
        let build_log: mage_build::template::LogFn = {
            let log_lines = log_lines.clone();
            std::rc::Rc::new(move |msg: &str| {
                let mut lines = log_lines.borrow_mut();
                lines.push(msg.to_string());
                let start = lines.len().saturating_sub(8);
                let view = lines[start..].join("\n");
                let _ = update_tx.send(mage_core::types::ToolUpdate { text: view });
            })
        };

        let log_msg = |msg: &str| { build_log(msg); };

        // Step 1: Generate workspace (sync — fast, just file I/O).
        log_msg("preparing workspace...");

        let (workspace_dir, cargo_path) = if let Some(root) = mage_build::template::find_workspace_root() {
            match mage_build::template::MageBuild::new(&root)
                .standard_extension_dirs()
                .with_log(build_log.clone())
                .generate_workspace()
            {
                Ok(r) => r,
                Err(e) => return ToolResult::failure(format!("Workspace generation failed: {e}")),
            }
        } else {
            // Snapshot path: extract + generate.
            let snapshot = mage_core::upgrade::get_snapshot();
            if snapshot.is_empty() {
                return ToolResult::failure("No workspace and no embedded snapshot.");
            }
            let module_dirs = standard_module_dirs();
            match mage_build::template::prepare_from_snapshot(
                snapshot, &module_dirs, &force_local,
            ) {
                Ok(r) => r,
                Err(e) => return ToolResult::failure(format!("Snapshot preparation failed: {e}")),
            }
        };

        // Step 2: Run cargo build asynchronously, streaming stderr.
        log_msg("compiling...");

        let mut child = match tokio::process::Command::new(&cargo_path)
            .arg("build")
            .arg("--message-format=json")
            .current_dir(&workspace_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::failure(format!("Failed to spawn cargo: {e}")),
        };

        // Stream stderr line by line to the tool widget.
        if let Some(stderr) = child.stderr.take() {
            let mut reader = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                // Filter out JSON diagnostic lines (they're noisy).
                if !line.starts_with('{') {
                    let clean = line.replace('\t', "    ");
                    log_msg(&clean);
                }
            }
        }

        let status = match child.wait().await {
            Ok(s) => s,
            Err(e) => return ToolResult::failure(format!("Cargo wait failed: {e}")),
        };

        if !status.success() {
            let msg = log_lines.borrow().join("\n");
            let truncated = if msg.len() > 3000 {
                format!("...(truncated)\n{}", &msg[msg.len() - 3000..])
            } else {
                msg
            };
            return ToolResult::failure(format!("Compilation failed:\n{truncated}"));
        }

        // Step 3: Find the output binary.
        let binary = find_binary(&workspace_dir);
        let path = match binary {
            Some(p) => {
                // Copy to ~/.mage/bin/
                let dest_dir = mage_build::default_approot().join("bin");
                let _ = std::fs::create_dir_all(&dest_dir);
                let dest = dest_dir.join(p.file_name().unwrap_or_default());
                let _ = std::fs::copy(&p, &dest);
                dest
            }
            None => return ToolResult::failure("Compilation succeeded but no binary found"),
        };

        log_msg(&format!("binary: {}", path.display()));

        match mage_core::upgrade::signal_upgrade(&path) {
            Ok(mage_core::upgrade::UpgradeSignal::Ready) => {
                ToolResult::success(format!("Compiled {}. Restarting...", path.display()))
            }
            Ok(mage_core::upgrade::UpgradeSignal::NoMonitor) => {
                ToolResult::success(format!(
                    "Compiled {}. Restart mage to use it.", path.display()
                ))
            }
            Err(e) => ToolResult::failure(format!("Upgrade signal failed: {e}")),
        }
    }
}

fn find_binary(workspace_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let target_dir = workspace_dir.join("target/debug");
    std::fs::read_dir(&target_dir).ok().and_then(|entries| {
        entries.filter_map(|e| e.ok()).find(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let p = e.path();
            p.is_file()
                && !name.ends_with(".d")
                && !name.ends_with(".rmeta")
                && !name.ends_with(".rlib")
                && !name.contains("build-script")
                && name.contains("mage")
        })
    }).map(|e| e.path())
}

fn standard_module_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".mage/modules"));
    }
    dirs
}
