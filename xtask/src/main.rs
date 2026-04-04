use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use mage_build::bundle::Config;
use mage_build::error::Result;
use mage_build::template::MageBuild;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    match args.first().map(|s| s.as_str()) {
        Some("bootstrap") => run(bootstrap),
        Some("brew") => run(brew),
        Some("help") | None => {
            print_help();
            ExitCode::SUCCESS
        }
        Some(cmd) => {
            eprintln!("error: unknown command '{cmd}'");
            print_help();
            ExitCode::FAILURE
        }
    }
}

fn run(f: fn() -> Result<()>) -> ExitCode {
    match f() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    eprintln!("usage: cargo xtask <command>");
    eprintln!();
    eprintln!("commands:");
    eprintln!("  bootstrap   Build generation-zero mage binary from workspace sources");
    eprintln!("  brew        Generate Homebrew formula for mage");
}

fn workspace_root() -> PathBuf {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::current_dir().unwrap());

    let mut dir = manifest_dir.as_path();
    loop {
        let candidate = dir.join("Cargo.toml");
        if candidate.exists() {
            let content = std::fs::read_to_string(&candidate).unwrap_or_default();
            if content.contains("[workspace]") {
                return dir.to_path_buf();
            }
        }
        dir = match dir.parent() {
            Some(p) => p,
            None => return env::current_dir().unwrap(),
        };
    }
}

fn bootstrap() -> Result<()> {
    let root = workspace_root();
    eprintln!("workspace root: {}", root.display());

    let result = MageBuild::new(&root)
        .name("mage-bootstrap")
        .config(Config {
            approot: root.join("target/mage-bootstrap"),
            ..Config::default()
        })
        .extension_dir(root.join("modules"))
        .compile()?;

    if result.success {
        if let Some(ref bin) = result.executable_path {
            eprintln!("success: {}", bin.display());
        }
    } else {
        eprintln!("compilation failed:\n{}", result.format_errors());
        return Err(mage_build::error::Error::Bundle("bootstrap failed".into()));
    }

    Ok(())
}

fn brew() -> Result<()> {
    let root = workspace_root();
    let formula_path = root.join("dist").join("mage.rb");
    std::fs::create_dir_all(formula_path.parent().unwrap())?;

    let formula = r##"class Mage < Formula
  desc "Self-replicating AI coding agent"
  homepage "https://github.com/semtexzv/mage"
  license "MIT"
  depends_on "rust" => :build

  def install
    system "cargo", "install", "--path", "xtask", "--root", buildpath/"xtask-bin"
    system buildpath/"xtask-bin/bin/xtask", "bootstrap"
    mage_bin = Dir[buildpath/".mage/bin/mage-*"].first
    if mage_bin
      bin.install mage_bin => "mage"
    else
      odie "Bootstrap failed"
    end
  end

  test do
    assert_match "mage", shell_output("#{bin}/mage --version 2>&1", 1)
  end
end
"##;

    std::fs::write(&formula_path, formula)?;
    eprintln!("wrote {}", formula_path.display());
    Ok(())
}
