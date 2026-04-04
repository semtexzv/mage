//! `mage snapshot` subcommand — inspect or extract the embedded snapshot.
//!
//! Usage:
//!   mage snapshot list          — list files in the snapshot
//!   mage snapshot extract <dir> — extract to a directory

/// Called from the generated main.rs to register the embedded snapshot.
pub fn set_snapshot(data: &'static [u8]) {
    mage_core::upgrade::set_snapshot(data);
}

/// Get the embedded snapshot data.
pub fn get_snapshot_data() -> &'static [u8] {
    mage_core::upgrade::get_snapshot()
}

pub fn run_snapshot(args: &[String]) {
    let subcmd = args.first().map(|s| s.as_str()).unwrap_or("list");

    let data = get_snapshot_data();
    if data.is_empty() {
        eprintln!("no embedded snapshot (binary was not built via mage-build)");
        std::process::exit(1);
    }

    match subcmd {
        "list" | "ls" => {
            match mage_build::template::list_snapshot(data) {
                Ok(entries) => {
                    for entry in &entries {
                        println!("{entry}");
                    }
                    eprintln!(
                        "\n{} entries, {:.1} KB compressed",
                        entries.len(),
                        data.len() as f64 / 1024.0
                    );
                }
                Err(e) => {
                    eprintln!("error reading snapshot: {e}");
                    std::process::exit(1);
                }
            }
        }
        "extract" => {
            let dest = args.get(1).map(|s| s.as_str()).unwrap_or("mage-snapshot");
            let dest = std::path::Path::new(dest);
            if dest.exists() {
                eprintln!("error: destination already exists: {}", dest.display());
                std::process::exit(1);
            }
            match mage_build::template::extract_snapshot(data, dest) {
                Ok(()) => eprintln!("extracted to {}", dest.display()),
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
        other => {
            eprintln!("unknown snapshot subcommand: {other}");
            eprintln!("usage: mage snapshot [list|extract <dir>]");
            std::process::exit(1);
        }
    }
}
