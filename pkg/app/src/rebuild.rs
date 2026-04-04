//! `mage rebuild` subcommand — recompile the binary with current modules.
//!
//! Two modes:
//! - **Workspace mode**: if a mage workspace is found, uses `MageBuild` (full path deps)
//! - **Snapshot mode**: if no workspace, uses the embedded snapshot to rebuild
//!
//! Both scan `~/.mage/modules/` and `.mage/modules/` for user-authored modules.

use std::path::PathBuf;

use mage_build::template::{find_workspace_root, MageBuild};

/// Run the rebuild subcommand.
pub fn run_rebuild() {
    let module_dirs = standard_module_dirs();

    let result = if let Some(root) = find_workspace_root() {
        eprintln!("rebuilding from workspace: {}", root.display());
        MageBuild::new(&root)
            .standard_extension_dirs()
            .compile()
    } else {
        eprintln!("no workspace found, rebuilding from embedded snapshot...");
        let snapshot = crate::snapshot_cmd::get_snapshot_data();
        if snapshot.is_empty() {
            eprintln!("error: no embedded snapshot and no workspace found");
            std::process::exit(1);
        }
        mage_build::template::compile_from_snapshot_data(snapshot, &module_dirs)
    };

    let result = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("rebuild failed: {e}");
            std::process::exit(1);
        }
    };

    if !result.success {
        eprintln!("compilation failed:\n{}", result.format_errors());
        std::process::exit(1);
    }

    let path = match result.executable_path {
        Some(p) => p,
        None => {
            eprintln!("compilation succeeded but no binary path returned");
            std::process::exit(1);
        }
    };

    eprintln!("compiled: {}", path.display());

    match mage_core::upgrade::signal_upgrade(&path) {
        Ok(mage_core::upgrade::UpgradeSignal::Ready) => {
            std::process::exit(mage_core::upgrade::UPGRADE_EXIT_CODE);
        }
        Ok(mage_core::upgrade::UpgradeSignal::NoMonitor) => {
            eprintln!("not running under monitor — restart mage to use the new binary");
        }
        Err(e) => {
            eprintln!("upgrade signal failed: {e}");
            std::process::exit(1);
        }
    }
}

fn standard_module_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".mage/modules"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".mage/modules"));
    }
    dirs
}
