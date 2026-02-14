use std::env;
use std::process::Command;

use tempfile::tempdir;

use metarust::bundle::{Bundle, Config, RenderContext, Template};
use metarust::error::Result as MrResult;
use metarust::module::Dependency;
use metarust::deps::DepSpec;

struct AgentTemplate {
    generation: usize,
}

impl Template for AgentTemplate {
    fn render_main(&self, _ctx: &RenderContext) -> MrResult<String> {
        let code = r##"
use metarust::bundle::{Bundle, Config, Snapshot, Template};
use std::path::PathBuf;

const HISTORY: &[u8] = include_bytes!("history.json");

struct NextGenTemplate {
    generation: usize,
}

impl Template for NextGenTemplate {
    fn render_main(&self, _ctx: &metarust::bundle::RenderContext) -> metarust::error::Result<String> {
        let content = include_str!("main.rs");
        Ok(content.replace(
            &format!("let current_generation = {};", self.generation - 1),
            &format!("let current_generation = {};", self.generation)
        ))
    }

    fn render_dependencies(&self, _ctx: &metarust::bundle::RenderContext) -> metarust::error::Result<Vec<metarust::module::Dependency>> {
        let metarust_dir = std::env::var("METARUST_DIR").unwrap();
        Ok(vec![
            metarust::module::Dependency::External {
                name: "metarust".to_string(),
                spec: metarust::deps::DepSpec::parse(&format!(r#"{{ path = "{}" }}"#, metarust_dir)).unwrap(),
            },
            metarust::module::Dependency::External {
                name: "serde".to_string(),
                spec: metarust::deps::DepSpec::parse(r#"{ version = "1.0", features = ["derive"] }"#).unwrap(),
            },
            metarust::module::Dependency::External {
                name: "serde_json".to_string(),
                spec: metarust::deps::DepSpec::Version("1.0".to_string()),
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
    let metarust_dir = PathBuf::from(std::env::var("METARUST_DIR").unwrap());

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
        let metarust_dir = env::current_dir().unwrap();
        Ok(vec![
            Dependency::External {
                name: "metarust".to_string(),
                spec: DepSpec::parse(&format!(r#"{{ path = "{}" }}"#, metarust_dir.display())).unwrap(),
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
    let metarust_dir = env::current_dir().unwrap();

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
        .env("METARUST_DIR", metarust_dir.to_str().unwrap())
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
