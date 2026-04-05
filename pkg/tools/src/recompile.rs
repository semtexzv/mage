//! Recompile tool — lets the agent trigger self-recompilation.
//!
//! Uses `MageBuild` if a workspace is found, otherwise falls back
//! to the embedded snapshot. Signals the monitor to upgrade on success.

use std::rc::Rc;

use async_trait::async_trait;
use serde_json::json;

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
                description: "Recompile the agent binary. Modules from ~/.mage/modules/ \
                    listed in force_local override snapshot modules with the same name. \
                    Unlisted local modules are added as new. After compilation under \
                    the monitor, the agent restarts with the new binary.".into(),
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

        let module_dirs = standard_module_dirs();

        ctx.send_text("Compiling...");

        // Try workspace first, then snapshot
        let result = if let Some(root) = mage_build::template::find_workspace_root() {
            mage_build::template::MageBuild::new(&root)
                .standard_extension_dirs()
                .compile()
        } else {
            let snapshot = mage_core::upgrade::get_snapshot();
            if snapshot.is_empty() {
                return ToolResult::failure(
                    "No workspace and no embedded snapshot — cannot recompile.",
                );
            }
            mage_build::template::compile_from_snapshot_data(
                snapshot, &module_dirs, &force_local,
            )
        };

        let result = match result {
            Ok(r) => r,
            Err(e) => return ToolResult::failure(format!("Build setup failed: {e}")),
        };

        if !result.success {
            let mut msg = String::from("Compilation failed:\n");
            let errors = result.format_errors();
            if !errors.is_empty() {
                msg.push_str(&errors);
            }
            if !result.cargo_stderr.is_empty() {
                if !errors.is_empty() {
                    msg.push_str("\n\n");
                }
                // Truncate stderr to last 3000 chars to fit in context.
                let stderr = &result.cargo_stderr;
                if stderr.len() > 3000 {
                    msg.push_str("...(truncated)\n");
                    msg.push_str(&stderr[stderr.len() - 3000..]);
                } else {
                    msg.push_str(stderr);
                }
            }
            return ToolResult::failure(msg);
        }

        let path = match result.executable_path {
            Some(p) => p,
            None => return ToolResult::failure("No binary path returned"),
        };

        match mage_core::upgrade::signal_upgrade(&path) {
            Ok(mage_core::upgrade::UpgradeSignal::Ready) => {
                mage_core::upgrade::safe_exit(mage_core::upgrade::UPGRADE_EXIT_CODE);
            }
            Ok(mage_core::upgrade::UpgradeSignal::NoMonitor) => ToolResult::success(format!(
                "Compiled new binary at {}. \
                 Not running under monitor — restart mage to use it.",
                path.display()
            )),
            Err(e) => ToolResult::failure(format!("Upgrade signal failed: {e}")),
        }
    }
}

fn standard_module_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".mage/modules"));
    }
    dirs
}
