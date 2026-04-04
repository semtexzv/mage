//! Recompile tool — lets the agent trigger self-recompilation.
//!
//! Scans extension modroots, compiles a new binary via `MageBuild`,
//! and signals the monitor to upgrade (exit 42). If not under a monitor,
//! returns the path as a tool result.

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
                description: "Recompile the agent binary with any new or modified extensions \
                    from ~/.mage/modules/ and .mage/modules/. After compilation under \
                    the monitor, the agent process restarts with the new binary. Use after \
                    writing a new extension file.".into(),
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
        let workspace_root = match mage_build::template::find_workspace_root() {
            Some(r) => r,
            None => return ToolResult::failure(
                "Cannot find mage workspace root. Set MAGE_WORKSPACE_ROOT.",
            ),
        };

        let result = match mage_build::template::MageBuild::new(&workspace_root)
            .standard_extension_dirs()
            .compile()
        {
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
            Ok(mage_core::upgrade::UpgradeSignal::NoMonitor) => {
                ToolResult::success(format!(
                    "Compiled new binary at {}. \
                     Not running under monitor — restart mage to use it.",
                    path.display()
                ))
            }
            Err(e) => ToolResult::failure(format!("Upgrade signal failed: {e}")),
        }
    }
}
