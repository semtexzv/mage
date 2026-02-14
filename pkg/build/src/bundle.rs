use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;

use sha2::{Digest, Sha256};

use crate::compile::{
    CompilationDiagnostics, CompilationResult, Diagnostic, DiagnosticCode, parse_dep_info,
};
use crate::deps::{ConflictStrategy, DepOrigin, DepSpec, DependencyResolver};
use crate::error::{Error, Result};
use crate::module::{Dependency, Module};
use crate::toolchain::Toolchain;

/// Configuration for `metarust`
#[derive(Debug, Clone)]
pub struct Config {
    pub approot: PathBuf,
    /// How to handle conflicting version requirements from different sources.
    pub conflict_strategy: ConflictStrategy,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            approot: crate::default_approot(),
            conflict_strategy: ConflictStrategy::default(),
        }
    }
}

/// A trait for rendering the entry-point template.
pub struct RenderContext<'a> {
    pub modules: &'a [Module],
}

pub trait Template {
    /// Renders the entry point `main.rs` based on the discovered modules.
    ///
    /// # Errors
    /// Returns an error if the template rendering fails.
    fn render_main(&self, ctx: &RenderContext) -> Result<String>;

    /// Optionally returns additional Cargo dependencies for the entry point itself.
    ///
    /// These are merged with module-declared dependencies through the `DependencyResolver`,
    /// which provides semver-aware deduplication and conflict detection.
    ///
    /// # Errors
    /// Returns an error if the dependency rendering fails.
    fn render_dependencies(&self, _ctx: &RenderContext) -> Result<Vec<Dependency>> {
        Ok(Vec::new())
    }
}


/// Represents the source of a patched Cargo dependency.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum PatchSource {
    Path(PathBuf),
    Git {
        repo: String,
        branch: Option<String>,
        rev: Option<String>,
        tag: Option<String>,
    },
}

impl std::fmt::Display for PatchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(p) => write!(f, "{{ path = \"{}\" }}", p.display()),
            Self::Git {
                repo,
                branch,
                rev,
                tag,
            } => {
                write!(f, "{{ git = {repo:?}")?;
                if let Some(b) = branch {
                    write!(f, ", branch = {b:?}")?;
                }
                if let Some(r) = rev {
                    write!(f, ", rev = {r:?}")?;
                }
                if let Some(t) = tag {
                    write!(f, ", tag = {t:?}")?;
                }
                write!(f, " }}")
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub modules: HashMap<String, String>,
    pub template_main: String,
    #[serde(default)]
    pub core_libs: HashMap<String, String>,
}

pub struct Bundle {
    pub id: String,
    pub config: Config,
    pub modules: Vec<Module>,
    pub shared_libs: Vec<PathBuf>,
    pub core_crates: Vec<PathBuf>,
    pub patches: HashMap<String, HashMap<String, PatchSource>>,
    pub assets: HashMap<PathBuf, Vec<u8>>,
    pub template: Option<Box<dyn Template>>,
    pub toolchain: Option<Toolchain>,
}

impl Bundle {
    /// Creates a new, empty Bundle with the given ID.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            config: Config::default(),
            modules: Vec::new(),
            shared_libs: Vec::new(),
            core_crates: Vec::new(),
            patches: HashMap::new(),
            assets: HashMap::new(),
            template: None,
            toolchain: None,
        }
    }

    /// Overrides the default configuration.
    #[must_use]
    pub fn with_config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }

    /// Explicitly sets the Toolchain to use for compilation.
    #[must_use]
    pub fn with_toolchain(mut self, toolchain: Toolchain) -> Self {
        self.toolchain = Some(toolchain);
        self
    }

    /// Adds a pre-parsed Module to the bundle.
    #[must_use]
    pub fn add_module(mut self, module: Module) -> Self {
        self.modules.push(module);
        self
    }

    /// Adds a shared library path.
    #[must_use]
    pub fn add_shared(mut self, path: PathBuf) -> Self {
        self.shared_libs.push(path);
        self
    }

    /// Adds a core crate path.
    #[must_use]
    pub fn add_core_crate(mut self, path: PathBuf) -> Self {
        self.core_crates.push(path);
        self
    }

    /// Adds a crate patch.
    #[must_use]
    pub fn add_patch(
        mut self,
        registry: impl Into<String>,
        crate_name: impl Into<String>,
        patch_source: PatchSource,
    ) -> Self {
        let registry = registry.into();
        self.patches
            .entry(registry)
            .or_default()
            .insert(crate_name.into(), patch_source);
        self
    }

    /// Adds an ad-hoc asset to be written into the generated workspace.
    #[must_use]
    pub fn add_asset(mut self, path: impl Into<PathBuf>, data: Vec<u8>) -> Self {
        self.assets.insert(path.into(), data);
        self
    }

    /// Sets the meta-template to use for assembly.
    #[must_use]
    pub fn with_template<T: Template + 'static>(mut self, template: T) -> Self {
        self.template = Some(Box::new(template));
        self
    }

    /// Generates the Cargo workspace structure on disk.
    ///
    /// # Errors
    /// Returns an error if no template is set, or if file I/O fails.
    pub fn generate(&self) -> Result<()> {
        let template = self
            .template
            .as_ref()
            .ok_or_else(|| Error::Bundle("No template provided for Bundle".to_string()))?;

        let workspace_dir = self.config.approot.join("workspaces").join(&self.id);
        let src_dir = workspace_dir.join("src");

        fs::create_dir_all(&src_dir)?;

        // Generate the entry-point src/main.rs
        let ctx = RenderContext {
            modules: &self.modules,
        };
        let main_source = template.render_main(&ctx)?;
        fs::write(src_dir.join("main.rs"), main_source)?;

        // Generate Cargo.toml
        let cargo_toml_content = self.generate_cargo_toml(template.as_ref())?;
        fs::write(workspace_dir.join("Cargo.toml"), cargo_toml_content)?;

        // Write ad-hoc assets into src/ so they can be included via include_bytes!
        for (asset_path, data) in &self.assets {
            let dest_path = src_dir.join(asset_path);
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(dest_path, data)?;
        }

        Ok(())
    }

    /// Returns a code-level snapshot of the current bundle.
    ///
    /// # Errors
    /// Returns an error if no template is set, or if module sources cannot be read.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let template = self
            .template
            .as_ref()
            .ok_or_else(|| Error::Bundle("No template provided for Bundle".to_string()))?;

        let ctx = RenderContext {
            modules: &self.modules,
        };
        let template_main = template.render_main(&ctx)?;

        let mut modules_map = HashMap::new();
        for module in &self.modules {
            if module.is_dir {
                // If it's a directory module, read all .rs files
                let parent_dir = module.path.parent().ok_or_else(|| {
                    Error::Bundle(format!(
                        "Directory module '{}' has no parent directory",
                        module.name
                    ))
                })?;
                #[allow(clippy::redundant_closure_for_method_calls)]
                for entry in walkdir::WalkDir::new(parent_dir)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    let path = entry.path();
                    if path.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
                        let content = fs::read_to_string(path)?;
                        let rel_path = path
                            .strip_prefix(parent_dir)
                            .unwrap_or(path)
                            .to_string_lossy()
                            .to_string();
                        modules_map.insert(format!("{}/{rel_path}", module.name), content);
                    }
                }
            } else if module.path.exists() {
                let content = fs::read_to_string(&module.path)?;
                modules_map.insert(module.name.clone(), content);
            } else {
                return Err(Error::Bundle(format!(
                    "Module source file not found: {}",
                    module.path.display()
                )));
            }
        }

        let mut core_libs = HashMap::new();
        for crate_dir in &self.core_crates {
            let cargo_toml = crate_dir.join("Cargo.toml");
            let pkg_name = if cargo_toml.exists() {
                Self::get_package_name(&cargo_toml)?
            } else {
                crate_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default()
            };
            let lib_rs = crate_dir.join("src/lib.rs");
            let main_rs = crate_dir.join("src/main.rs");
            let content = if lib_rs.exists() {
                fs::read_to_string(&lib_rs)?
            } else if main_rs.exists() {
                fs::read_to_string(&main_rs)?
            } else {
                String::new()
            };
            core_libs.insert(pkg_name, content);
        }
        Ok(Snapshot {
            modules: modules_map,
            template_main,
            core_libs,
        })
    }

    /// Computes a stable SHA-256 content hash of the bundle.
    ///
    /// The hash incorporates (in deterministic order): the bundle ID, each module's
    /// source content (sorted by name), the rendered `main.rs` and dependencies
    /// (if a template is set), the toolchain paths/target, and all asset content
    /// (sorted by path).
    ///
    /// # Errors
    /// Returns an error if module source files cannot be read or template rendering fails.
    pub fn content_hash(&self) -> Result<String> {
        let mut hasher = Sha256::new();

        // 1. Bundle ID
        hasher.update(self.id.as_bytes());

        // 2. Module source content, sorted by name
        let mut module_entries: Vec<(&str, &Module)> = self
            .modules
            .iter()
            .map(|m| (m.name.as_str(), m))
            .collect();
        module_entries.sort_by_key(|(name, _)| *name);
        for (name, module) in &module_entries {
            hasher.update(name.as_bytes());
            if module.path.exists() {
                let content = fs::read_to_string(&module.path)?;
                hasher.update(content.as_bytes());
            }
        }

        // 3 & 4. Rendered main.rs and dependencies (if template set)
        if let Some(ref template) = self.template {
            let ctx = RenderContext {
                modules: &self.modules,
            };
            let main_source = template.render_main(&ctx)?;
            hasher.update(main_source.as_bytes());

            let deps = template.render_dependencies(&ctx)?;
            let mut dep_strs: Vec<String> = deps
                .iter()
                .map(|d| format!("{d:?}"))
                .collect();
            dep_strs.sort();
            for s in &dep_strs {
                hasher.update(s.as_bytes());
            }
        }

        // 5. Toolchain version info
        if let Some(ref tc) = self.toolchain {
            hasher.update(tc.rustc_path.to_string_lossy().as_bytes());
            hasher.update(tc.cargo_path.to_string_lossy().as_bytes());
            if let Some(ref target) = tc.target {
                hasher.update(target.as_bytes());
            }
        }

        // 6. Asset content, sorted by path
        let mut asset_keys: Vec<&PathBuf> = self.assets.keys().collect();
        asset_keys.sort();
        for key in asset_keys {
            hasher.update(key.to_string_lossy().as_bytes());
            hasher.update(&self.assets[key]);
        }

        let digest = hasher.finalize();
        Ok(format!("{digest:x}"))
    }

    /// Compiles the generated workspace and copies the resulting binary.
    ///
    /// # Errors
    /// Returns an error if compilation or binary copying fails.
    pub fn compile(&self) -> Result<CompilationResult> {
        let workspace_dir = self.config.approot.join("workspaces").join(&self.id);

        let toolchain = match &self.toolchain {
            Some(t) => t.clone(),
            None => Toolchain::resolve_system()?,
        };

        let toolchain_metadata = toolchain.extract_metadata()?;

        let mut child = Self::spawn_cargo(&toolchain, &workspace_dir)?;
        let mut diagnostics = Self::process_diagnostics(&mut child, &self.id)?;

        let status = child.wait()?;
        let success = status.success();

        if !success && diagnostics.errors.is_empty() {
            let mut cargo_error_msg = String::new();
            let mut capturing = false;

            for line in diagnostics.cargo_stderr.lines() {
                if line.starts_with("error: ")
                    || line.starts_with("fatal: ")
                    || line.contains("panicked at")
                {
                    capturing = true;
                }

                if capturing {
                    cargo_error_msg.push_str(line);
                    cargo_error_msg.push('\n');
                }
            }

            if cargo_error_msg.trim().is_empty() && !diagnostics.cargo_stderr.trim().is_empty() {
                cargo_error_msg = diagnostics.cargo_stderr.trim().to_string();
            }

            if !cargo_error_msg.trim().is_empty() {
                let is_crash = cargo_error_msg.contains("panicked at");
                let message = if is_crash {
                    "Cargo crashed unexpectedly"
                } else {
                    "Cargo build failed"
                };

                diagnostics.errors.push(Diagnostic {
                    message: message.to_string(),
                    code: None,
                    level: "error".to_string(),
                    rendered: Some(cargo_error_msg.trim().to_string()),
                });
            }
        }

        let final_bin_path = if success {
            if let Some(src_bin) = diagnostics.artifact_path {
                Some(Self::copy_output_binary(
                    self,
                    &src_bin,
                )?)
            } else {
                None
            }
        } else {
            None
        };

        // Try to parse the dep-info file for source dependencies.
        let dep_info_files = if success {
            Self::find_and_parse_dep_info(&workspace_dir, &self.id, toolchain.target.as_deref())
        } else {
            Vec::new()
        };

        Ok(CompilationResult {
            success,
            executable_path: final_bin_path,
            errors: diagnostics.errors,
            warnings: diagnostics.warnings,
            cargo_stderr: diagnostics.cargo_stderr,
            toolchain_metadata,
            dep_info_files,
        })
    }

    fn get_package_name(cargo_toml_path: &Path) -> Result<String> {
        let content = fs::read_to_string(cargo_toml_path)?;
        let parsed: toml::Value = toml::from_str(&content).map_err(|e| {
            Error::Bundle(format!(
                "Failed to parse Cargo.toml at {}: {e}",
                cargo_toml_path.display()
            ))
        })?;

        parsed
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .map(str::to_string)
            .ok_or_else(|| {
                Error::Bundle(format!(
                    "Could not find package name in {}",
                    cargo_toml_path.display()
                ))
            })
    }

    /// Resolves a list of crate directories into a `name → canonicalized path` map.
    fn resolve_crate_dirs(dirs: &[PathBuf]) -> Result<HashMap<String, PathBuf>> {
        let mut map = HashMap::new();
        for dir in dirs {
            let path = fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
            let pkg_name = Self::get_package_name(&path.join("Cargo.toml"))?;
            map.insert(pkg_name, path);
        }
        Ok(map)
    }

    fn generate_cargo_toml(&self, template: &dyn Template) -> Result<String> {
        let resolved_shared = Self::resolve_crate_dirs(&self.shared_libs)?;
        let resolved_core = Self::resolve_crate_dirs(&self.core_crates)?;
        let workspace_dir = self.config.approot.join("workspaces").join(&self.id);
        // Build the root TOML document
        let mut doc = toml::Table::new();
        // [package]
        let mut package = toml::Table::new();
        package.insert("name".into(), toml::Value::String(self.id.clone()));
        package.insert("version".into(), toml::Value::String("0.1.0".into()));
        package.insert("edition".into(), toml::Value::String("2021".into()));
        doc.insert("package".into(), toml::Value::Table(package));

        // [workspace]
        let mut workspace = toml::Table::new();
        let all_members: Vec<&PathBuf> = resolved_shared
            .values()
            .chain(resolved_core.values())
            .collect();
        if !all_members.is_empty() {
            let members: Vec<toml::Value> = all_members
                .iter()
                .map(|path| {
                    let rel = pathdiff::diff_paths(path, &workspace_dir)
                        .unwrap_or_else(|| (*path).clone());
                    toml::Value::String(rel.to_string_lossy().replace('\\', "/"))
                })
                .collect();
            workspace.insert("members".into(), toml::Value::Array(members));
        }
        doc.insert("workspace".into(), toml::Value::Table(workspace));

        // Collect all dependencies through DependencyResolver
        let mut resolver = DependencyResolver::new(self.config.conflict_strategy);

        // Core crates — always injected as path deps
        for (name, path) in &resolved_core {
            let rel = pathdiff::diff_paths(path, &workspace_dir).unwrap_or_else(|| path.clone());
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let mut t = toml::Table::new();
            t.insert("path".into(), toml::Value::String(rel_str));
            resolver.add(name.clone(), DepSpec::Full(t), DepOrigin::Core)?;
        }

        // Template-provided deps
        let ctx = RenderContext {
            modules: &self.modules,
        };
        let template_deps = template.render_dependencies(&ctx)?;
        for dep in &template_deps {
            match dep {
                Dependency::External { name, spec } => {
                    resolver.add(name.clone(), spec.clone(), DepOrigin::Template)?;
                }
                Dependency::Internal(name) => {
                    if let Some(lib_path) = resolved_shared.get(name) {
                        let rel = pathdiff::diff_paths(lib_path, &workspace_dir)
                            .unwrap_or_else(|| lib_path.clone());
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        let mut t = toml::Table::new();
                        t.insert("path".into(), toml::Value::String(rel_str));
                        resolver.add(name.clone(), DepSpec::Full(t), DepOrigin::Template)?;
                    } else {
                        return Err(Error::Bundle(format!(
                            "Template declares internal dependency '{name}' but it was not found \
                             in shared libs or core crates"
                        )));
                    }
                }
            }
        }
        // Module-declared deps
        for module in &self.modules {
            let origin = DepOrigin::Module(module.name.clone());
            for dep in &module.dependencies {
                match dep {
                    Dependency::External { name, spec } => {
                        resolver.add(name.clone(), spec.clone(), origin.clone())?;
                    }
                    Dependency::Internal(name) => {
                        if let Some(lib_path) = resolved_shared.get(name) {
                            let rel = pathdiff::diff_paths(lib_path, &workspace_dir)
                                .unwrap_or_else(|| lib_path.clone());
                            let rel_str = rel.to_string_lossy().replace('\\', "/");
                            let mut t = toml::Table::new();
                            t.insert("path".into(), toml::Value::String(rel_str));
                            resolver.add(name.clone(), DepSpec::Full(t), origin.clone())?;
                        } else {
                            return Err(Error::Bundle(format!(
                                "Module '{}' declares internal dependency '{name}' but it was \
                                 not found in shared libs or core crates",
                                module.name
                            )));
                        }
                    }
                }
            }
        }

        // Build the [dependencies] table from resolved deps
        let mut deps_table = toml::Table::new();
        for resolved_dep in resolver.resolved() {
            deps_table.insert(resolved_dep.name, resolved_dep.spec.to_toml_value());
        }
        doc.insert("dependencies".into(), toml::Value::Table(deps_table));
        // [patch.*]
        if !self.patches.is_empty() {
            let mut patch_table = toml::Table::new();
            for (registry, crates) in &self.patches {
                let mut registry_table = toml::Table::new();
                for (crate_name, config) in crates {
                    // Parse PatchSource display into TOML value
                    let patch_str = format!("v = {config}");
                    let parsed: toml::Table = toml::from_str(&patch_str).map_err(|e| {
                        Error::Bundle(format!("Invalid patch for '{crate_name}': {e}"))
                    })?;
                    if let Some(val) = parsed.get("v") {
                        registry_table.insert(crate_name.clone(), val.clone());
                    }
                }
                patch_table.insert(registry.clone(), toml::Value::Table(registry_table));
            }
            doc.insert("patch".into(), toml::Value::Table(patch_table));
        }

        toml::to_string_pretty(&doc).map_err(|e| {
            Error::Bundle(format!("Failed to serialize Cargo.toml: {e}"))
        })
    }
}

/// Recursively copies all files and directories from `src` to `dst`.
///
/// # Errors
/// Returns an I/O error if any file or directory operation fails.
pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let dest = dst.as_ref().join(entry.file_name());
        // Follow symlinks: use metadata() which follows symlinks, not symlink_metadata()
        let meta = fs::metadata(entry.path())?;
        if meta.is_dir() {
            copy_dir_all(entry.path(), dest)?;
        } else {
            fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

impl Bundle {
    fn spawn_cargo(toolchain: &Toolchain, workspace_dir: &Path) -> Result<Child> {
        let mut cargo_args = vec!["build", "--message-format=json"];
        if let Some(ref target) = toolchain.target {
            cargo_args.push("--target");
            cargo_args.push(target);
        }

        Command::new(&toolchain.cargo_path)
            .args(&cargo_args)
            .current_dir(workspace_dir)
            .env("RUSTC", &toolchain.rustc_path)
            .env("CARGO", &toolchain.cargo_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Compilation(format!("Failed to spawn cargo build process: {e}")))
    }

    fn process_diagnostics(child: &mut Child, bundle_id: &str) -> Result<CompilationDiagnostics> {
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        // Spawn a thread to completely consume stderr to avoid deadlocking the child process.
        let stderr_thread = thread::spawn(move || {
            let mut s = String::new();
            let mut reader = BufReader::new(stderr);
            let _ = reader.read_to_string(&mut s);
            s
        });

        let reader = BufReader::new(stdout);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut artifact_path = None;

        for line in BufRead::lines(reader) {
            let line = line?;
            if let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) {
                if msg["reason"] == "compiler-message" {
                    if let Some(message) = msg.get("message") {
                        let level = message["level"].as_str().unwrap_or("").to_string();
                        let diag = Diagnostic {
                            message: message["message"].as_str().unwrap_or("").to_string(),
                            code: message
                                .get("code")
                                .and_then(|c| c.get("code"))
                                .and_then(|c| c.as_str())
                                .map(|s| DiagnosticCode {
                                    code: s.to_string(),
                                }),
                            level: level.clone(),
                            rendered: message["rendered"].as_str().map(str::to_string),
                        };

                        if level == "error" {
                            errors.push(diag);
                        } else if level == "warning" {
                            warnings.push(diag);
                        }
                    }
                } else if msg["reason"] == "compiler-artifact" {
                    if let Some(target) = msg.get("target") {
                        if target["name"].as_str() == Some(bundle_id) {
                            if let Some(executable) = msg.get("executable").and_then(|e| e.as_str()) {
                                artifact_path = Some(PathBuf::from(executable));
                            }
                        }
                    }
                }
            }
        }

        // Join the stderr thread.
        let cargo_stderr = stderr_thread.join().unwrap_or_default();

        Ok(CompilationDiagnostics {
            errors,
            warnings,
            artifact_path,
            cargo_stderr,
        })
    }

    /// Generates a human-friendly `adjective-animal` suffix from the bundle's content hash.
    ///
    /// Uses the SHA-256 content hash (not just the bundle ID) so that different code
    /// produces different names, while identical code always maps to the same name.
    /// The petname `small` word list provides 449 adjectives × 452 nouns ≈ 203K
    /// combinations — more than enough for practical use. Collisions are theoretically
    /// possible but vanishingly rare, and the usability of `my-app-curious-falcon` over
    /// `my-app-a1b2c3d4e5f6` is worth the trade-off.
    fn readable_suffix(bundle: &Bundle) -> String {
        // Hash the full content when available, fall back to just the ID.
        let digest = match bundle.content_hash() {
            Ok(hex) => {
                // Re-derive raw bytes from the hex string.
                Sha256::digest(hex.as_bytes())
            }
            Err(_) => Sha256::digest(bundle.id.as_bytes()),
        };
        let petnames = petname::Petnames::small();
        let adj_idx = u16::from_be_bytes([digest[0], digest[1]]) as usize % petnames.adjectives.len();
        let noun_idx = u16::from_be_bytes([digest[2], digest[3]]) as usize % petnames.nouns.len();
        format!(
            "{}-{}",
            petnames.adjectives[adj_idx], petnames.nouns[noun_idx]
        )
    }

    /// Locates and parses the `.d` dep-info file produced by cargo after a successful build.
    ///
    /// Returns an empty vec if the file is not found or cannot be parsed.
    fn find_and_parse_dep_info(
        workspace_dir: &Path,
        bundle_id: &str,
        target_triple: Option<&str>,
    ) -> Vec<PathBuf> {
        // Cargo writes dep-info as `target/{profile}/{name}.d`.
        // For cross-compilation it's `target/{triple}/{profile}/{name}.d`.
        // Hyphens in the package name are replaced with underscores in the filename.
        let dep_name = bundle_id.replace('-', "_");

        let debug_dir = if let Some(triple) = target_triple {
            workspace_dir.join("target").join(triple).join("debug")
        } else {
            workspace_dir.join("target").join("debug")
        };

        let dep_file = debug_dir.join(format!("{dep_name}.d"));
        if dep_file.exists() {
            parse_dep_info(&dep_file).unwrap_or_default()
        } else {
            // Also try the original name (unhyphenated case).
            let dep_file_orig = debug_dir.join(format!("{bundle_id}.d"));
            if dep_file_orig.exists() {
                parse_dep_info(&dep_file_orig).unwrap_or_default()
            } else {
                Vec::new()
            }
        }
    }

    fn copy_output_binary(bundle: &Bundle, src_bin: &Path) -> Result<PathBuf> {
        let bin_dir = bundle.config.approot.join("bin");
        fs::create_dir_all(&bin_dir)?;
        let suffix = Self::readable_suffix(bundle);
        let dest_bin = bin_dir.join(format!("{}-{suffix}", bundle.id));
        fs::copy(src_bin, &dest_bin)?;
        Ok(dest_bin)
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use tempfile::tempdir;

    use super::*;
    use crate::module::Dependency;
    use crate::deps::DepSpec;

    struct SimpleTemplate;

    impl Template for SimpleTemplate {
        fn render_main(&self, ctx: &RenderContext) -> Result<String> {
            let mut out = String::new();
            for module in ctx.modules {
                let _ = writeln!(out, "// module: {}", module.name);
            }
            out.push_str("fn main() {}\n");
            Ok(out)
        }

        fn render_dependencies(&self, _ctx: &RenderContext) -> Result<Vec<Dependency>> {
            Ok(vec![Dependency::External {
                name: "tokio".to_string(),
                spec: DepSpec::parse(r#"{ version = "1.0", features = ["full"] }"#).unwrap(),
            }])
        }
    }

    #[test]
    fn test_bundle_generate() {
        let dir = tempdir().unwrap();
        let config = Config {
            approot: dir.path().to_path_buf(),
            ..Config::default()
        };

        // Create a dummy module file
        let module_path = dir.path().join("my_module.rs");
        fs::write(&module_path, "fn hook() {}").unwrap();

        let module = Module {
            name: "my_module".to_string(),
            path: module_path,
            modroot: None,
            relative_path: None,
            dependencies: vec![Dependency::External {
                name: "anyhow".to_string(),
                spec: DepSpec::Version("1.0".into()),
            }],
            init: None,
            is_dir: false,
        };

        let bundle = Bundle::new("test-bundle")
            .with_config(config)
            .add_module(module)
            .with_template(SimpleTemplate);

        assert!(bundle.generate().is_ok());

        // Check workspace dir
        let workspace_dir = dir.path().join("workspaces/test-bundle");
        assert!(workspace_dir.exists());

        // Check main.rs
        let main_rs = workspace_dir.join("src/main.rs");
        assert!(main_rs.exists());
        let main_content = fs::read_to_string(&main_rs).unwrap();
        assert!(main_content.contains("// module: my_module"));
        assert!(main_content.contains("fn main() {}"));

        // Check Cargo.toml
        let cargo_toml = workspace_dir.join("Cargo.toml");
        assert!(cargo_toml.exists());
        let toml_content = fs::read_to_string(&cargo_toml).unwrap();

        assert!(toml_content.contains("[package]"));
        assert!(toml_content.contains(r#"name = "test-bundle""#));
        assert!(toml_content.contains(r#"version = "0.1.0""#));
        assert!(toml_content.contains("[dependencies.tokio]"));
        assert!(toml_content.contains(r#"version = "1.0""#));
        assert!(toml_content.contains(r#"anyhow = "1.0""#));
    }

    #[test]
    fn test_bundle_shared_packages() {
        let dir = tempdir().unwrap();
        let config = Config {
            approot: dir.path().to_path_buf(),
            ..Config::default()
        };

        // Create a dummy shared library
        let shared_lib_dir = dir.path().join("core_shared_lib");
        fs::create_dir_all(&shared_lib_dir).unwrap();
        let shared_cargo_toml = shared_lib_dir.join("Cargo.toml");
        fs::write(
            &shared_cargo_toml,
            "\n[package]\nname = \"core_shared_lib\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        // Create a dummy module file that depends on the shared lib
        let module_path = dir.path().join("my_module.rs");
        fs::write(&module_path, "fn hook() {}").unwrap();

        let module = Module {
            name: "my_module".to_string(),
            path: module_path,
            modroot: None,
            relative_path: None,
            dependencies: vec![Dependency::Internal("core_shared_lib".to_string())],
            init: None,
            is_dir: false,
        };

        let bundle = Bundle::new("test-bundle")
            .with_config(config)
            .add_module(module)
            .add_shared(shared_lib_dir)
            .with_template(SimpleTemplate);

        bundle.generate().unwrap();

        // Check Cargo.toml
        let workspace_dir = dir.path().join("workspaces/test-bundle");
        let cargo_toml = workspace_dir.join("Cargo.toml");
        let toml_content = fs::read_to_string(&cargo_toml).unwrap();

        // It should include the path dependency
        assert!(toml_content.contains("[dependencies.core_shared_lib]"));

        // And it should include it in the members
        assert!(toml_content.contains("members = ["));
        assert!(toml_content.contains("core_shared_lib"));
    }

    #[test]
    fn test_bundle_core_crates() {
        let dir = tempdir().unwrap();
        let config = Config {
            approot: dir.path().to_path_buf(),
            ..Config::default()
        };

        // Create a dummy core crate
        let core_crate_dir = dir.path().join("agent_core");
        fs::create_dir_all(&core_crate_dir).unwrap();
        let core_cargo_toml = core_crate_dir.join("Cargo.toml");
        fs::write(
            &core_cargo_toml,
            "\n[package]\nname = \"agent_core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();

        // Create a dummy module file that DOES NOT depend on the core crate
        let module_path = dir.path().join("my_module.rs");
        fs::write(&module_path, "fn hook() {}").unwrap();

        let module = Module {
            name: "my_module".to_string(),
            path: module_path,
            modroot: None,
            relative_path: None,
            dependencies: vec![],
            init: None,
            is_dir: false,
        };

        let bundle = Bundle::new("test-bundle")
            .with_config(config)
            .add_module(module)
            .add_core_crate(core_crate_dir)
            .with_template(SimpleTemplate);

        assert!(bundle.generate().is_ok());

        // Check Cargo.toml
        let workspace_dir = dir.path().join("workspaces/test-bundle");
        let cargo_toml = workspace_dir.join("Cargo.toml");
        let toml_content = fs::read_to_string(&cargo_toml).unwrap();

        // It should automatically inject the core crate into dependencies
        assert!(toml_content.contains("[dependencies.agent_core]"));

        // And it should automatically include it in the members
        assert!(toml_content.contains("members = ["));
        assert!(toml_content.contains("agent_core"));
    }
}
