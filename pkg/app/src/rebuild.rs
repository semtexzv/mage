//! `mage rebuild` subcommand — recompile the binary with current modules.
//!
//! Scans `~/.mage/modules/` and `.mage/modules/` for user-authored modules,
//! compiles a new binary, and signals the monitor to upgrade (or prints
//! the path if standalone).

use mage_build::template::{MageBuild, find_workspace_root};

/// Run the rebuild subcommand.
pub fn run_rebuild() {
    let workspace_root = match find_workspace_root() {
        Some(r) => r,
        None => {
            eprintln!("error: cannot find mage workspace root");
            eprintln!("set MAGE_WORKSPACE_ROOT or run from within the workspace");
            std::process::exit(1);
        }
    };

    eprintln!("workspace: {}", workspace_root.display());

    let result = match MageBuild::new(&workspace_root)
        .standard_extension_dirs()
        .compile()
    {
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
