//! Monitor process — supervises the agent and handles self-recompilation.
//!
//! The mage binary has two modes determined by the `MAGE_AGENT_PIPE_FD` env var:
//!
//! - **Not set** → This process is the monitor. It spawns itself as a child
//!   with the pipe FD set, then watches for exit code 42 (upgrade signal).
//!
//! - **Set** → This process is the agent. Run normally.
//!
//! ## Upgrade protocol
//!
//! 1. Agent compiles a new binary via mage-build
//! 2. Agent writes the new binary path to the pipe (one line)
//! 3. Agent exits with code 42
//! 4. Monitor reads the path, spawns the new binary
//! 5. If the new binary exits non-42, monitor exits with that code
//! 6. If it exits 42 again, the chain continues

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

pub use mage_core::upgrade::{PIPE_ENV, UPGRADE_EXIT_CODE, is_agent_mode, signal_upgrade};

/// Run as the monitor. Spawns the current binary as a child with a pipe,
/// handles exit code 42 for upgrades.
///
/// Returns the final exit code.
pub fn run_monitor() -> ExitCode {
    let mut current_binary = std::env::current_exe().unwrap_or_else(|_| {
        PathBuf::from(std::env::args().next().unwrap_or_else(|| "mage".to_string()))
    });

    // Forward all args to the child.
    let args: Vec<String> = std::env::args().skip(1).collect();

    loop {
        let result = spawn_with_pipe(&current_binary, &args);

        match result {
            SpawnResult::Exit(code) => {
                return if code == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(code as u8)
                };
            }
            SpawnResult::Upgrade(new_binary) => {
                // Clear terminal between old and new binary.
                print!("\x1b[2J\x1b[H\x1b[3J");
                use std::io::Write;
                let _ = std::io::stdout().flush();
                if !new_binary.exists() {
                    eprintln!(
                        "[monitor] error: new binary does not exist: {}",
                        new_binary.display()
                    );
                    return ExitCode::FAILURE;
                }
                current_binary = new_binary;
                // Loop: spawn the new binary with the same args.
            }
            SpawnResult::Error(msg) => {
                eprintln!("[monitor] error: {msg}");
                return ExitCode::FAILURE;
            }
        }
    }
}

enum SpawnResult {
    /// Child exited normally (non-42).
    Exit(i32),
    /// Child exited 42 and wrote a new binary path.
    Upgrade(PathBuf),
    /// Failed to spawn or communicate.
    Error(String),
}

fn spawn_with_pipe(binary: &PathBuf, args: &[String]) -> SpawnResult {
    // Create a pipe: child writes to it, we read from it.
    // On Unix we use an anonymous pipe via os_pipe.
    // For simplicity (and portability), use stdin/stdout redirection:
    // we pipe the child's FD 3 to ourselves. But that's complex.
    //
    // Simpler approach: use a temp file. The child writes the path,
    // we read it after the child exits. This is less elegant but
    // avoids platform-specific pipe FD inheritance.
    let pipe_path = std::env::temp_dir().join(format!("mage-upgrade-{}", std::process::id()));

    // Ensure the file exists and is empty.
    let _ = std::fs::write(&pipe_path, "");

    let mut cmd = Command::new(binary);
    cmd.args(args);
    cmd.env(PIPE_ENV, pipe_path.display().to_string());

    // Inherit stdio so the child's TUI works.
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return SpawnResult::Error(format!("failed to spawn {}: {e}", binary.display())),
    };

    let status = match child.wait() {
        Ok(s) => s,
        Err(e) => return SpawnResult::Error(format!("failed to wait: {e}")),
    };

    let exit_code = status.code().unwrap_or(1);

    if exit_code == UPGRADE_EXIT_CODE {
        // Read the upgrade path from the pipe file.
        match std::fs::read_to_string(&pipe_path) {
            Ok(content) => {
                let _ = std::fs::remove_file(&pipe_path);
                let path = content.trim().to_string();
                if path.is_empty() {
                    SpawnResult::Error("child exited 42 but wrote no path".into())
                } else {
                    SpawnResult::Upgrade(PathBuf::from(path))
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&pipe_path);
                SpawnResult::Error(format!("failed to read upgrade pipe: {e}"))
            }
        }
    } else {
        let _ = std::fs::remove_file(&pipe_path);
        SpawnResult::Exit(exit_code)
    }
}

