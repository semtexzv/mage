use std::env;
use std::process::Command;

use tempfile::tempdir;

use mage_build::bundle::{Bundle, Config, RenderContext, Template};
use mage_build::error::Result as MrResult;
use mage_build::module::Dependency;
use mage_build::deps::DepSpec;

struct AgentTemplate {
    generation: usize,
}

impl Template for AgentTemplate {
    fn render_main(&self, _ctx: &RenderContext) -> MrResult<String> {
        let code = r##"
use mage_build::bundle::{Bundle, Config, Snapshot, Template};
use std::path::PathBuf;

const HISTORY: &[u8] = include_bytes!("history.json");

struct NextGenTemplate {
    generation: usize,
}

impl Template for NextGenTemplate {
    fn render_main(&self, _ctx: &mage_build::bundle::RenderContext) -> mage_build::error::Result<String> {
        let content = include_str!("main.rs");
        Ok(content.replace(
            &format!("let current_generation = {};", self.generation - 1),
            &format!("let current_generation = {};", self.generation)
        ))
    }

    fn render_dependencies(&self, _ctx: &mage_build::bundle::RenderContext) -> mage_build::error::Result<Vec<mage_build::module::Dependency>> {
        let mage_build_dir = std::env::var("MAGE_BUILD_DIR").unwrap();
        Ok(vec![
            mage_build::module::Dependency::External {
                name: "mage_build".to_string(),
                spec: mage_build::deps::DepSpec::parse(&format!(r#"{{ path = "{}" }}"#, mage_build_dir)).unwrap(),
            },
            mage_build::module::Dependency::External {
                name: "serde".to_string(),
                spec: mage_build::deps::DepSpec::parse(r#"{ version = "1.0", features = ["derive"] }"#).unwrap(),
            },
            mage_build::module::Dependency::External {
                name: "serde_json".to_string(),
                spec: mage_build::deps::DepSpec::Version("1.0".to_string()),
            },
        ])
    }
}

fn main() {
    let current_generation = {GEN};
    println!("I am generation {}", current_generation);

    let mut history: Vec<Snapshot> = if HISTORY.is_empty() {
        Vec::new()
    } else {
        serde_json::from_slice(HISTORY).unwrap()
    };

    println!("History size: {}", history.len());

    if current_generation >= 3 {
        println!("Generation 3 reached, printing source of Generation 1");
        let gen1 = &history[0];
        println!("Gen1 main:\n{}", gen1.template_main);
        return;
    }

    let approot = PathBuf::from(std::env::var("AGENT_APPROOT").unwrap());
    let mage_build_dir = PathBuf::from(std::env::var("MAGE_BUILD_DIR").unwrap());

    let bundle = Bundle::new(format!("agent-gen-{}", current_generation + 1))
        .with_config(Config { approot, ..Config::default() })
        .with_template(NextGenTemplate { generation: current_generation + 1 });
    let snapshot = bundle.snapshot().unwrap();
    history.push(snapshot);

    let new_history = serde_json::to_vec(&history).unwrap();

    let bundle = bundle.add_asset("history.json", new_history);
    bundle.generate().unwrap();

    let result = bundle.compile().unwrap();
    assert!(result.success, "Compilation failed for gen {}", current_generation + 1);

    let bin_path = result.executable_path.unwrap();
    println!("Compiled next generation at: {:?}", bin_path);

    // Execute the next generation
    let status = std::process::Command::new(&bin_path)
        .status()
        .unwrap();

    assert!(status.success());
}"##;
        Ok(code.replace("{GEN}", &self.generation.to_string()))
    }

    fn render_dependencies(&self, _ctx: &RenderContext) -> MrResult<Vec<Dependency>> {
        let mage_build_dir = env::current_dir().unwrap();
        Ok(vec![
            Dependency::External {
                name: "mage_build".to_string(),
                spec: DepSpec::parse(&format!(r#"{{ path = "{}" }}"#, mage_build_dir.display())).unwrap(),
            },
            Dependency::External {
                name: "serde".to_string(),
                spec: DepSpec::parse(r#"{ version = "1.0", features = ["derive"] }"#).unwrap(),
            },
            Dependency::External {
                name: "serde_json".to_string(),
                spec: DepSpec::Version("1.0".to_string()),
            },
        ])
    }
}

#[test]
fn test_recursive_agent() {
    let dir = tempdir().unwrap();
    let approot = dir.path().to_path_buf();
    let mage_build_dir = env::current_dir().unwrap();

    let config = Config {
        approot: approot.clone(),
        ..Config::default()
    };

    let bundle = Bundle::new("agent-gen-1")
        .with_config(config)
        .with_template(AgentTemplate { generation: 1 })
        .add_asset("history.json", b"".to_vec());

    bundle.generate().unwrap();
    let result = bundle.compile().unwrap();
    if !result.success {
        for err in &result.errors {
            println!("Error: {}", err.message);
        }
        panic!("Compilation of generation 1 failed");
    }

    let bin_path = result.executable_path.unwrap();

    // Execute generation 1
    let output = Command::new(&bin_path)
        .env("AGENT_APPROOT", approot.to_str().unwrap())
        .env("MAGE_BUILD_DIR", mage_build_dir.to_str().unwrap())
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();

    println!("STDOUT:\n{stdout}");
    println!("STDERR:\n{stderr}");

    assert!(output.status.success(), "Execution of generation 1 failed");
    assert!(stdout.contains("I am generation 1"));
    assert!(stdout.contains("I am generation 2"));
    assert!(stdout.contains("I am generation 3"));
    assert!(stdout.contains("Generation 3 reached, printing source of Generation 1"));
    // check if it prints the original code (or some part of it)
    assert!(stdout.contains("Gen1 main:"));
}
