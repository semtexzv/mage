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
                description: "Recompile the agent binary with any new or modified modules \
                    from ~/.mage/modules/ and .mage/modules/. After compilation under \
                    the monitor, the agent process restarts with the new binary. Use after \
                    writing a new module file.".into(),
                parameters: json!({
                    "type": "object",
                    "properties": {},
                }),
            },
            handler: Rc::new(RecompileHandler),
        }]
    }
}

struct RecompileHandler;

#[async_trait(?Send)]
impl ToolHandler for RecompileHandler {
    async fn execute(&self, _args: serde_json::Value, _ctx: ToolContext) -> ToolResult {
        let module_dirs = standard_module_dirs();

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
            mage_build::template::compile_from_snapshot_data(snapshot, &module_dirs)
        };

        let result = match result {
            Ok(r) => r,
            Err(e) => return ToolResult::failure(format!("Compilation failed: {e}")),
        };

        if !result.success {
            return ToolResult::failure(format!(
                "Compilation failed:\n{}",
                result.format_errors()
            ));
        }

        let path = match result.executable_path {
            Some(p) => p,
            None => return ToolResult::failure("No binary path returned"),
        };

        match mage_core::upgrade::signal_upgrade(&path) {
            Ok(mage_core::upgrade::UpgradeSignal::Ready) => {
                std::process::exit(mage_core::upgrade::UPGRADE_EXIT_CODE);
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
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".mage/modules"));
    }
    dirs
}
