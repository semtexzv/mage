use std::fmt::Write as _;
use std::fs;

use mage_build::bundle::{Bundle, Config, RenderContext, Template};
use mage_build::error::Result as MrResult;
use mage_build::module::Module;

struct SimpleTemplate;

impl Template for SimpleTemplate {
    fn render_main(&self, ctx: &RenderContext) -> MrResult<String> {
        let mut out = String::new();
        for module in ctx.modules {
            // Include the staged module using #[path]
            let _ = writeln!(out, "#[path = \"{}\"]", module.path.display());
            let _ = writeln!(out, "mod {};", module.name);
        }
        out.push_str("fn main() {\n");
        for module in ctx.modules {
            let _ = writeln!(out, "    {}::init();", module.name);
        }
        out.push_str("    println!(\"Hello Metarust!\");\n}\n");
        Ok(out)
    }
}

#[test]
fn test_bundle_compilation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = Config {
        approot: temp_dir.path().to_path_buf(),
        ..Config::default()
    };

    let module_path = temp_dir.path().join("my_plugin.rs");
    fs::write(
        &module_path,
        "pub fn init() { println!(\"Init my_plugin\"); }",
    )
    .unwrap();

    let module = Module {
        name: "my_plugin".to_string(),
        path: module_path,
        modroot: None,
        relative_path: None,
        dependencies: vec![],
        init: None,
        is_dir: false,
    };

    let bundle = Bundle::new("test_bundle")
        .with_config(config)
        .add_module(module)
        .with_template(SimpleTemplate);

    bundle.generate().unwrap();

    let compile_result = bundle.compile().unwrap();
    assert!(
        compile_result.success,
        "Compilation should succeed, output:\n{compile_result}",
    );

    assert!(compile_result.executable_path.is_some());
    let bin_path = compile_result.executable_path.unwrap();
    assert!(bin_path.exists());

    assert!(bin_path.starts_with(temp_dir.path().join("bin")));
}

#[test]
fn test_bundle_compilation_failure() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = Config {
        approot: temp_dir.path().to_path_buf(),
        ..Config::default()
    };

    let module_path = temp_dir.path().join("bad_plugin.rs");
    // This module has invalid rust code
    fs::write(&module_path, "pub fn init() { unknown_function(); }").unwrap();

    let module = Module {
        name: "bad_plugin".to_string(),
        path: module_path,
        modroot: None,
        relative_path: None,
        dependencies: vec![],
        init: None,
        is_dir: false,
    };

    let bundle = Bundle::new("bad_bundle")
        .with_config(config)
        .add_module(module)
        .with_template(SimpleTemplate);

    bundle.generate().unwrap();

    let compile_result = bundle.compile().unwrap();
    assert!(!compile_result.success, "Compilation should fail");

    assert!(
        !compile_result.errors.is_empty(),
        "Should capture at least one error"
    );

    let formatted_errors = compile_result.format_errors();
    assert!(
        formatted_errors.contains("cannot find function `unknown_function` in this scope"),
        "Error message should contain details: {formatted_errors}",
    );

    assert!(
        compile_result.executable_path.is_none(),
        "No executable should be copied"
    );
}

#[test]
fn test_print_bin_path() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = Config {
        approot: temp_dir.path().to_path_buf(),
        ..Config::default()
    };

    let module_path = temp_dir.path().join("my_plugin.rs");
    fs::write(&module_path, "pub fn init() {}").unwrap();
    let module = Module {
        name: "my_plugin".to_string(),
        path: module_path,
        modroot: None,
        relative_path: None,
        dependencies: vec![],
        init: None,
        is_dir: false,
    };
    let bundle = Bundle::new("test_bundle_name")
        .with_config(config)
        .add_module(module)
        .with_template(SimpleTemplate);
    bundle.generate().unwrap();
    let compile_result = bundle.compile().unwrap();
    println!(
        "Compiled executable path: {:?}",
        compile_result.executable_path
    );
}
