//! Module extension model.
//!
//! Each module is a single `.rs` file or a `mod.rs`-rooted directory that can
//! optionally declare a `pub fn init(...)` function as its entry-point hook.
//! The `init` function is discovered via `syn` AST parsing of **top-level items
//! only** — functions nested inside `impl` blocks or sub-modules are ignored.
//! When present, the generated `main.rs` calls `init` to initialize the module.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use quote::ToTokens;
use regex::Regex;

use crate::deps::DepSpec;
use crate::error::{Error, Result};

#[derive(Debug, PartialEq, Clone, serde::Serialize)]
pub enum Dependency {
    /// External Cargo dependency, e.g., `anyhow = "1.0"`.
    /// `spec` is a structured representation of the TOML dependency value.
    External { name: String, spec: DepSpec },
    /// Internal dependency on another single-file module.
    Internal(String),
}

/// A structured representation of a parsed function signature,
/// derived directly from `syn::Signature`.
///
/// This is used to capture the shape of hook functions (such as `init`) so that
/// template-based code generation can emit correct calling code in `main.rs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FunctionSignature {
    pub name: String,
    pub is_async: bool,
    pub is_pub: bool,
    pub inputs: Vec<FunctionArg>,
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FunctionArg {
    pub name: String,
    pub ty: String,
    pub is_mut: bool,
    pub is_ref: bool,
}

impl FunctionSignature {
    #[must_use]
    pub fn from_syn(sig: &syn::Signature, is_pub: bool) -> Self {
        let name = sig.ident.to_string();
        let is_async = sig.asyncness.is_some();

        let mut inputs = Vec::new();
        for arg in &sig.inputs {
            if let syn::FnArg::Typed(pat_type) = arg {
                let mut is_mut = false;
                let mut is_ref = false;
                let mut arg_name = String::new();

                if let syn::Pat::Ident(pat_ident) = &*pat_type.pat {
                    arg_name = pat_ident.ident.to_string();
                    is_mut = pat_ident.mutability.is_some();
                }

                if let syn::Type::Reference(type_ref) = &*pat_type.ty {
                    is_ref = true;
                    if type_ref.mutability.is_some() {
                        is_mut = true;
                    }
                }

                inputs.push(FunctionArg {
                    name: arg_name,
                    ty: pat_type.ty.to_token_stream().to_string(),
                    is_mut,
                    is_ref,
                });
            }
        }

        let output = match &sig.output {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, ty) => Some(ty.to_token_stream().to_string()),
        };

        Self {
            name,
            is_async,
            is_pub,
            inputs,
            output,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Module {
    pub name: String,
    pub path: PathBuf,
    /// The root directory where this module was discovered (if known)
    pub modroot: Option<PathBuf>,
    /// The path of this module relative to its modroot (if known)
    pub relative_path: Option<PathBuf>,
    pub dependencies: Vec<Dependency>,
    /// The module's `init` entry-point hook, if one was found.
    ///
    /// `init` is the extension mechanism's entry-point hook. It **must** be a
    /// top-level function (not inside an `impl` block or nested module). If
    /// present, the generated `main.rs` will call this function to initialize
    /// the module at startup.
    pub init: Option<FunctionSignature>,
    pub is_dir: bool,
}

static DEP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*//\s*@dep\s+(.+)$").unwrap());

impl Module {
    /// Parses a module from a file on disk.
    ///
    /// # Errors
    /// Returns an error if the file cannot be read or contains invalid syntax.
    pub fn parse_file(path: &Path, name: &str) -> Result<Self> {
        let content = fs::read_to_string(path)?;
        Self::parse_source(&content, name, path)
    }

    /// Parses a module from source code content.
    ///
    /// # Errors
    /// Returns an error if the source has malformed `@dep` directives or invalid Rust syntax.
    pub fn parse_source(content: &str, name: &str, path: &Path) -> Result<Self> {
        let mut dependencies = Vec::new();
        let mut init_hook = None;

        for (line_idx, line) in content.lines().enumerate() {
            if let Some(captures) = DEP_RE.captures(line) {
                let dep_str = captures[1].trim();

                if dep_str.is_empty() {
                    return Err(Error::Parse(format!(
                        "Malformed dependency at line {}: empty '@dep' directive",
                        line_idx + 1
                    )));
                }

                // Check if it's an external dependency (contains '=')
                if let Some(eq_idx) = dep_str.find('=') {
                    let dep_name = dep_str[..eq_idx].trim().to_string();
                    let raw_spec = dep_str[eq_idx + 1..].trim();

                    if dep_name.is_empty() || raw_spec.is_empty() {
                        return Err(Error::Parse(format!(
                            "Malformed external dependency at line {}: missing name or version. Expected `// @dep name = \"version\"`",
                            line_idx + 1
                        )));
                    }

                    let spec = DepSpec::parse(raw_spec).map_err(|e| {
                        Error::Parse(format!(
                            "Invalid dependency spec at line {}: {e}",
                            line_idx + 1
                        ))
                    })?;
                    dependencies.push(Dependency::External {
                        name: dep_name,
                        spec,
                    });
                } else {
                    // Internal dependency
                    // Should just be a single token (the module name)
                    let parts: Vec<&str> = dep_str.split_whitespace().collect();
                    if parts.len() != 1 {
                        return Err(Error::Parse(format!(
                            "Malformed internal dependency at line {}: expected single module name, got '{dep_str}'",
                            line_idx + 1,
                        )));
                    }

                    dependencies.push(Dependency::Internal(parts[0].to_string()));
                }
            }
        }

        // Introspect the module source code using syn
        let ast = syn::parse_file(content).map_err(|e| {
            Error::Parse(format!("Failed to parse Rust syntax in module {name}: {e}"))
        })?;

        for item in ast.items {
            if let syn::Item::Fn(item_fn) = item {
                let is_pub = matches!(item_fn.vis, syn::Visibility::Public(..));
                let fn_name = item_fn.sig.ident.to_string();

                if fn_name == "init" {
                    init_hook = Some(FunctionSignature::from_syn(&item_fn.sig, is_pub));
                }
            }
        }

        Ok(Self {
            name: name.to_string(),
            path: path.to_path_buf(),
            modroot: None,
            relative_path: None,
            dependencies,
            init: init_hook,
            is_dir: path.file_name().and_then(|n| n.to_str()) == Some("mod.rs"),
        })
    }
}

pub struct ModuleResolver {
    pub modroots: Vec<PathBuf>,
    pub exclusions: Vec<String>,
}

impl Default for ModuleResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleResolver {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            modroots: Vec::new(),
            exclusions: Vec::new(),
        }
    }

    pub fn add_modroot<P: AsRef<Path>>(&mut self, dir: P) {
        self.modroots.push(dir.as_ref().to_path_buf());
    }

    pub fn exclude_plugin(&mut self, name: &str) {
        self.exclusions.push(name.to_string());
    }

    /// Resolves a module by its name, looking through the configured search directories.
    ///
    /// # Errors
    /// Returns an error if a found module source file cannot be parsed.
    pub fn resolve(&self, name: &str) -> Result<Option<Module>> {
        if self.exclusions.contains(&name.to_string()) {
            return Ok(None);
        }

        // Look for 'name.rs' or 'name/mod.rs' in each search directory
        for dir in &self.modroots {
            let direct_path = dir.join(format!("{name}.rs"));
            if direct_path.is_file() {
                let mut module = Module::parse_file(&direct_path, name)?;
                module.modroot = Some(dir.clone());
                module.relative_path = Some(PathBuf::from(format!("{name}.rs")));
                return Ok(Some(module));
            }

            let mod_path = dir.join(name).join("mod.rs");
            if mod_path.is_file() {
                let mut module = Module::parse_file(&mod_path, name)?;
                module.modroot = Some(dir.clone());
                module.relative_path = Some(PathBuf::from(name).join("mod.rs"));
                return Ok(Some(module));
            }
        }

        Ok(None)
    }
}

/// Scan a directory for `.rs` modules. Returns empty vec if dir doesn't exist.
///
/// This is a convenience for bootstrap — scans one directory (non-recursively)
/// for `.rs` files, parses each, and returns successfully parsed modules.
/// Parse failures are printed to stderr and skipped.
pub fn scan_directory(dir: &Path) -> Vec<Module> {
    let mut modules = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return modules;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "rs") {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            match Module::parse_file(&path, &name) {
                Ok(m) => modules.push(m),
                Err(e) => {
                    eprintln!("  warning: failed to parse {}: {e}", path.display());
                }
            }
        }
    }
    modules
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deps::DepSpec;

    #[test]
    fn test_parse_dependencies() {
        let source = r#"
            // @dep anyhow = "1.0"
            // @dep serde = { version = "1.0", features = ["derive"] }
            // @dep my_internal_plugin

            pub fn run() {
                println!("Hello world");
            }
        "#;

        let module = Module::parse_source(source, "test_plugin", Path::new("dummy.rs")).unwrap();

        assert_eq!(module.dependencies.len(), 3);
        assert_eq!(
            module.dependencies[0],
            Dependency::External {
                name: "anyhow".into(),
                spec: DepSpec::Version("1.0".into())
            }
        );
        assert_eq!(
            module.dependencies[1],
            Dependency::External {
                name: "serde".into(),
                spec: DepSpec::parse(r#"{ version = "1.0", features = ["derive"] }"#).unwrap()
            }
        );
        assert_eq!(
            module.dependencies[2],
            Dependency::Internal("my_internal_plugin".into())
        );
    }

    #[test]
    fn test_plugin_introspection() {
        let source = r"
            // some comment
            pub fn my_hook(a: i32, b: String) -> Result<(), Error> {}

            pub async fn async_hook(ctx: &Context) {}

            fn private_helper() {}

            fn init() -> bool {}

            pub fn main() {}
        ";

        let module = Module::parse_source(source, "test_plugin", Path::new("dummy.rs")).unwrap();

        assert!(module.init.is_some());
        let init = module.init.unwrap();
        assert_eq!(init.name, "init");
        assert!(!init.is_async);
        assert_eq!(init.output.as_deref(), Some("bool"));
    }

    #[test]
    fn test_parse_malformed_dependency() {
        let source = r"
            // @dep anyhow =
        ";

        let result = Module::parse_source(source, "test", Path::new("dummy.rs"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Malformed"));
    }

    #[test]
    fn test_plugin_resolver() {
        use tempfile::tempdir;

        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();

        let plugin_a = dir1.path().join("plugin_a.rs");
        let plugin_b = dir2.path().join("plugin_b.rs");

        fs::write(&plugin_a, "// @dep plugin_b\npub fn a() {}").unwrap();
        fs::write(&plugin_b, "pub fn b() {}").unwrap();

        let mut resolver = ModuleResolver::new();
        resolver.add_modroot(dir1.path());
        resolver.add_modroot(dir2.path());

        // Resolve plugin_a
        let pa = resolver
            .resolve("plugin_a")
            .unwrap()
            .expect("Should find plugin_a");
        assert_eq!(pa.name, "plugin_a");
        assert_eq!(pa.dependencies.len(), 1);

        // Resolve plugin_b
        let pb = resolver
            .resolve("plugin_b")
            .unwrap()
            .expect("Should find plugin_b");
        assert_eq!(pb.name, "plugin_b");

        // Resolve non-existent
        let pc = resolver.resolve("plugin_c").unwrap();
        assert!(pc.is_none());

        // Exclude plugin_a
        resolver.exclude_plugin("plugin_a");
        let pa_excluded = resolver.resolve("plugin_a").unwrap();
        assert!(pa_excluded.is_none(), "Should not resolve excluded module");
    }

    #[test]
    fn test_plugin_resolver_multi_file() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();

        // Setup flat module
        let plugin_a = dir.path().join("plugin_a.rs");
        fs::write(&plugin_a, "pub fn a() {}").unwrap();

        // Setup multi-file module
        let multi_dir = dir.path().join("plugin_b");
        fs::create_dir_all(&multi_dir).unwrap();
        let plugin_b_mod = multi_dir.join("mod.rs");
        let plugin_b_helper = multi_dir.join("helper.rs");
        fs::write(&plugin_b_mod, "pub mod helper; pub fn b() {}").unwrap();
        fs::write(&plugin_b_helper, "pub fn c() {}").unwrap();

        let mut resolver = ModuleResolver::new();
        resolver.add_modroot(dir.path());

        // Resolve flat file
        let pa = resolver
            .resolve("plugin_a")
            .unwrap()
            .expect("Should find plugin_a");
        assert_eq!(pa.name, "plugin_a");
        assert!(!pa.is_dir);

        // Resolve multi-file mod.rs
        let pb = resolver
            .resolve("plugin_b")
            .unwrap()
            .expect("Should find plugin_b");
        assert_eq!(pb.name, "plugin_b");
        assert!(pb.is_dir);
        assert!(pb.path.ends_with("plugin_b/mod.rs"));
    }
}
