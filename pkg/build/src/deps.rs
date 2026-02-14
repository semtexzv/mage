//! Dependency resolution, deduplication, and conflict detection.
//!
//! This module provides structured handling of Cargo dependencies collected from
//! multiple sources (core crates, template declarations, module `@dep` annotations).
//! Dependencies are merged with semver-aware deduplication: when two sources request
//! the same crate with compatible version ranges, the tighter constraint wins.
//! Incompatible ranges produce an error by default, or can be configured to pick
//! the latest range.

use std::collections::HashMap;
use std::fmt;

use crate::error::{Error, Result};

/// How to handle conflicting version requirements for the same crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictStrategy {
    /// Return an error on incompatible version requirements.
    #[default]
    Error,
    /// Pick the latest (highest minimum) version requirement.
    Latest,
}

/// A parsed, structured representation of a single Cargo dependency specification.
///
/// This mirrors the forms accepted in `Cargo.toml`:
/// - `crate = "1.0"` → `DepSpec::Version`
/// - `crate = { version = "1.0", features = [...] }` → `DepSpec::Full`
/// - `crate = { path = "..." }` → `DepSpec::Full`
/// - `crate = { git = "..." }` → `DepSpec::Full`
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DepSpec {
    /// A simple version string, e.g. `"1.0"`.
    Version(String),
    /// A full inline table specification (the parsed TOML table).
    Full(toml::Table),
}

impl DepSpec {
    /// Parses a raw TOML value string (the RHS of `name = <spec>`) into a `DepSpec`.
    ///
    /// # Errors
    /// Returns an error if the string is not valid TOML.
    pub fn parse(raw: &str) -> Result<Self> {
        // Wrap in a dummy assignment so we can parse it as a TOML document.
        let doc_str = format!("v = {raw}");
        let doc: toml::Table = toml::from_str(&doc_str).map_err(|e| {
            Error::Parse(format!("Invalid dependency spec '{raw}': {e}"))
        })?;
        let val = doc.get("v").cloned().ok_or_else(|| {
            Error::Parse(format!("Failed to extract parsed value from '{raw}'"))
        })?;

        match val {
            toml::Value::String(s) => Ok(Self::Version(s)),
            toml::Value::Table(t) => Ok(Self::Full(t)),
            other => Err(Error::Parse(format!(
                "Unexpected TOML type for dependency spec: {other}"
            ))),
        }
    }

    /// Extracts the version requirement string, if present.
    #[must_use]
    pub fn version_req(&self) -> Option<&str> {
        match self {
            Self::Version(v) => Some(v.as_str()),
            Self::Full(t) => t.get("version").and_then(|v| v.as_str()),
        }
    }

    /// Returns true if this spec is a path dependency.
    #[must_use]
    pub fn is_path(&self) -> bool {
        matches!(self, Self::Full(t) if t.contains_key("path"))
    }

    /// Returns true if this spec is a git dependency.
    #[must_use]
    pub fn is_git(&self) -> bool {
        matches!(self, Self::Full(t) if t.contains_key("git"))
    }

    /// Serializes this spec back to a TOML value suitable for `Cargo.toml`.
    #[must_use]
    pub fn to_toml_value(&self) -> toml::Value {
        match self {
            Self::Version(v) => toml::Value::String(v.clone()),
            Self::Full(t) => toml::Value::Table(t.clone()),
        }
    }
}

impl fmt::Display for DepSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(v) => write!(f, "\"{v}\""),
            Self::Full(t) => {
                let val = toml::Value::Table(t.clone());
                write!(f, "{}", toml::to_string_pretty(&val).unwrap_or_default().trim())
            }
        }
    }
}

/// Origin tracking for diagnostics — where did this dependency come from?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepOrigin {
    /// Injected from core crate configuration.
    Core,
    /// Declared by the template.
    Template,
    /// Declared by a module's `// @dep` annotation.
    Module(String),
}

impl fmt::Display for DepOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core => write!(f, "core crate"),
            Self::Template => write!(f, "template"),
            Self::Module(name) => write!(f, "module '{name}'"),
        }
    }
}

/// A resolved external dependency with its origin.
#[derive(Debug, Clone)]
pub struct ResolvedDep {
    pub name: String,
    pub spec: DepSpec,
    pub origin: DepOrigin,
}

/// Collects dependencies from multiple sources and resolves conflicts.
pub struct DependencyResolver {
    strategy: ConflictStrategy,
    /// name → (spec, origin)
    deps: HashMap<String, (DepSpec, DepOrigin)>,
}

impl DependencyResolver {
    #[must_use]
    pub fn new(strategy: ConflictStrategy) -> Self {
        Self {
            strategy,
            deps: HashMap::new(),
        }
    }

    /// Adds a dependency. If the name already exists, attempts to merge using
    /// semver-aware deduplication.
    ///
    /// # Errors
    /// Returns an error on incompatible versions when strategy is `Error`.
    pub fn add(&mut self, name: String, spec: DepSpec, origin: DepOrigin) -> Result<()> {
        if let Some((existing_spec, existing_origin)) = self.deps.get(&name) {
            // Path or git deps: must be identical
            if existing_spec.is_path() || existing_spec.is_git() || spec.is_path() || spec.is_git()
            {
                if existing_spec == &spec {
                    return Ok(()); // exact dedup
                }
                return Err(Error::Bundle(format!(
                    "Dependency conflict for '{name}': path/git dep from {existing_origin} \
                     cannot be merged with spec from {origin}"
                )));
            }

            // Both have version requirements — try semver merge
            let existing_ver = existing_spec.version_req();
            let new_ver = spec.version_req();

            match (existing_ver, new_ver) {
                (Some(ev), Some(nv)) => {
                    let merged = self.merge_versions(&name, ev, nv, existing_origin, &origin)?;
                    // Build the merged spec — keep the richer of the two specs
                    // (the one with more keys like features), but update its version.
                    let merged_spec = Self::merge_spec_with_version(
                        existing_spec, &spec, &merged,
                    );
                    let merged_origin = existing_origin.clone();
                    self.deps.insert(name, (merged_spec, merged_origin));
                }
                _ => {
                    // One or both lack a version — can't do semver merge
                    if existing_spec == &spec {
                        return Ok(());
                    }
                    return Err(Error::Bundle(format!(
                        "Dependency conflict for '{name}': incompatible specs from \
                         {existing_origin} and {origin} (cannot perform semver merge)"
                    )));
                }
            }
        } else {
            self.deps.insert(name, (spec, origin));
        }
        Ok(())
    }

    /// Returns all resolved dependencies, sorted by name for deterministic output.
    #[must_use]
    pub fn resolved(self) -> Vec<ResolvedDep> {
        let mut result: Vec<ResolvedDep> = self
            .deps
            .into_iter()
            .map(|(name, (spec, origin))| ResolvedDep { name, spec, origin })
            .collect();
        result.sort_by(|a, b| a.name.cmp(&b.name));
        result
    }

    /// Attempts to merge two semver version requirement strings.
    ///
    /// Strategy:
    /// 1. Parse both as `semver::VersionReq`.
    /// 2. Extract the minimum version each implies.
    /// 3. Check if the higher minimum satisfies both requirements.
    /// 4. If yes, return the tighter (higher-minimum) requirement.
    /// 5. If no, error or pick latest based on strategy.
    fn merge_versions(
        &self,
        name: &str,
        existing: &str,
        incoming: &str,
        existing_origin: &DepOrigin,
        incoming_origin: &DepOrigin,
    ) -> Result<String> {
        // If they're identical strings, short-circuit
        if existing == incoming {
            return Ok(existing.to_string());
        }

        let req_e = semver::VersionReq::parse(existing).map_err(|e| {
            Error::Parse(format!("Invalid semver for '{name}' from {existing_origin}: {e}"))
        })?;
        let req_i = semver::VersionReq::parse(incoming).map_err(|e| {
            Error::Parse(format!("Invalid semver for '{name}' from {incoming_origin}: {e}"))
        })?;

        // Extract minimum versions from requirements.
        let min_e = Self::min_version_from_req(&req_e);
        let min_i = Self::min_version_from_req(&req_i);

        match (min_e, min_i) {
            (Some(ve), Some(vi)) => {
                // Pick the higher minimum
                let (higher_min, higher_req_str, _lower_req) = if ve >= vi {
                    (&ve, existing, &req_i)
                } else {
                    (&vi, incoming, &req_e)
                };

                // Check if the higher minimum satisfies both requirements
                if req_e.matches(higher_min) && req_i.matches(higher_min) {
                    Ok(higher_req_str.to_string())
                } else {
                    match self.strategy {
                        ConflictStrategy::Error => Err(Error::Bundle(format!(
                            "Dependency conflict for '{name}': '{existing}' (from {existing_origin}) \
                             is incompatible with '{incoming}' (from {incoming_origin})"
                        ))),
                        ConflictStrategy::Latest => {
                            // Pick the one with the higher minimum
                            Ok(higher_req_str.to_string())
                        }
                    }
                }
            }
            _ => {
                // Can't extract minimum versions — fall back to string equality
                match self.strategy {
                    ConflictStrategy::Error => Err(Error::Bundle(format!(
                        "Dependency conflict for '{name}': '{existing}' (from {existing_origin}) \
                         vs '{incoming}' (from {incoming_origin}) — cannot determine compatibility"
                    ))),
                    ConflictStrategy::Latest => {
                        // Keep the existing one as a fallback
                        Ok(existing.to_string())
                    }
                }
            }
        }
    }

    /// Extracts the minimum version that would satisfy a `VersionReq`.
    ///
    /// For simple requirements like `^1.2.3` or `>=1.0, <2.0`, this extracts
    /// the first comparator's version. This is a heuristic, not exact.
    fn min_version_from_req(req: &semver::VersionReq) -> Option<semver::Version> {
        req.comparators.first().map(|c| {
            semver::Version::new(
                c.major,
                c.minor.unwrap_or(0),
                c.patch.unwrap_or(0),
            )
        })
    }

    /// Given two `DepSpec`s and a resolved version string, produce the merged spec.
    ///
    /// Prefers the spec with more keys (e.g., one with `features`) and updates
    /// its version field to the resolved version.
    fn merge_spec_with_version(a: &DepSpec, b: &DepSpec, version: &str) -> DepSpec {
        match (a, b) {
            // Both simple versions → simple version
            (DepSpec::Version(_), DepSpec::Version(_)) => DepSpec::Version(version.to_string()),
            // One is a table → prefer the table, merge features
            (DepSpec::Full(ta), DepSpec::Full(tb)) => {
                let mut merged = ta.clone();
                // Merge features arrays
                if let Some(toml::Value::Array(fb)) = tb.get("features") {
                    let entry = merged
                        .entry("features")
                        .or_insert_with(|| toml::Value::Array(Vec::new()));
                    if let toml::Value::Array(fa) = entry {
                        for f in fb {
                            if !fa.contains(f) {
                                fa.push(f.clone());
                            }
                        }
                    }
                }
                merged.insert(
                    "version".to_string(),
                    toml::Value::String(version.to_string()),
                );
                DepSpec::Full(merged)
            }
            (DepSpec::Full(t), DepSpec::Version(_)) | (DepSpec::Version(_), DepSpec::Full(t)) => {
                let mut merged = t.clone();
                merged.insert(
                    "version".to_string(),
                    toml::Value::String(version.to_string()),
                );
                DepSpec::Full(merged)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dep_spec_parse_simple_version() {
        let spec = DepSpec::parse(r#""1.0""#).unwrap();
        assert_eq!(spec, DepSpec::Version("1.0".to_string()));
        assert_eq!(spec.version_req(), Some("1.0"));
        assert!(!spec.is_path());
        assert!(!spec.is_git());
    }

    #[test]
    fn test_dep_spec_parse_full_table() {
        let spec = DepSpec::parse(r#"{ version = "1.0", features = ["derive"] }"#).unwrap();
        assert!(matches!(spec, DepSpec::Full(_)));
        assert_eq!(spec.version_req(), Some("1.0"));
        assert!(!spec.is_path());
    }

    #[test]
    fn test_dep_spec_parse_path() {
        let spec = DepSpec::parse(r#"{ path = "../foo" }"#).unwrap();
        assert!(spec.is_path());
        assert!(!spec.is_git());
        assert_eq!(spec.version_req(), None);
    }

    #[test]
    fn test_dep_spec_parse_git() {
        let spec = DepSpec::parse(r#"{ git = "https://github.com/foo/bar" }"#).unwrap();
        assert!(spec.is_git());
        assert!(!spec.is_path());
    }

    #[test]
    fn test_resolver_dedup_identical() {
        let mut r = DependencyResolver::new(ConflictStrategy::Error);
        r.add(
            "serde".into(),
            DepSpec::Version("1.0".into()),
            DepOrigin::Template,
        )
        .unwrap();
        r.add(
            "serde".into(),
            DepSpec::Version("1.0".into()),
            DepOrigin::Module("foo".into()),
        )
        .unwrap();
        let resolved = r.resolved();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].spec, DepSpec::Version("1.0".into()));
    }

    #[test]
    fn test_resolver_dedup_compatible_versions() {
        let mut r = DependencyResolver::new(ConflictStrategy::Error);
        r.add(
            "serde".into(),
            DepSpec::Version("1.0".into()),
            DepOrigin::Template,
        )
        .unwrap();
        // 1.0.228 is compatible with ^1.0, and is tighter
        r.add(
            "serde".into(),
            DepSpec::Version("1.0.228".into()),
            DepOrigin::Module("foo".into()),
        )
        .unwrap();
        let resolved = r.resolved();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].spec, DepSpec::Version("1.0.228".into()));
    }

    #[test]
    fn test_resolver_conflict_incompatible_versions() {
        let mut r = DependencyResolver::new(ConflictStrategy::Error);
        r.add(
            "serde".into(),
            DepSpec::Version("1.0".into()),
            DepOrigin::Template,
        )
        .unwrap();
        let err = r
            .add(
                "serde".into(),
                DepSpec::Version("2.0".into()),
                DepOrigin::Module("bar".into()),
            )
            .unwrap_err();
        assert!(err.to_string().contains("conflict"), "got: {err}");
    }

    #[test]
    fn test_resolver_conflict_latest_strategy() {
        let mut r = DependencyResolver::new(ConflictStrategy::Latest);
        r.add(
            "serde".into(),
            DepSpec::Version("1.0".into()),
            DepOrigin::Template,
        )
        .unwrap();
        r.add(
            "serde".into(),
            DepSpec::Version("2.0".into()),
            DepOrigin::Module("bar".into()),
        )
        .unwrap();
        let resolved = r.resolved();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].spec, DepSpec::Version("2.0".into()));
    }

    #[test]
    fn test_resolver_merge_features() {
        let mut r = DependencyResolver::new(ConflictStrategy::Error);
        r.add(
            "serde".into(),
            DepSpec::parse(r#"{ version = "1.0", features = ["derive"] }"#).unwrap(),
            DepOrigin::Template,
        )
        .unwrap();
        r.add(
            "serde".into(),
            DepSpec::parse(r#"{ version = "1.0", features = ["alloc"] }"#).unwrap(),
            DepOrigin::Module("foo".into()),
        )
        .unwrap();
        let resolved = r.resolved();
        assert_eq!(resolved.len(), 1);
        if let DepSpec::Full(t) = &resolved[0].spec {
            let features = t.get("features").unwrap().as_array().unwrap();
            let strs: Vec<&str> = features.iter().filter_map(|v| v.as_str()).collect();
            assert!(strs.contains(&"derive"));
            assert!(strs.contains(&"alloc"));
        } else {
            panic!("Expected Full spec");
        }
    }

    #[test]
    fn test_resolver_path_conflict() {
        let mut r = DependencyResolver::new(ConflictStrategy::Error);
        r.add(
            "mylib".into(),
            DepSpec::parse(r#"{ path = "../foo" }"#).unwrap(),
            DepOrigin::Core,
        )
        .unwrap();
        let err = r
            .add(
                "mylib".into(),
                DepSpec::parse(r#"{ path = "../bar" }"#).unwrap(),
                DepOrigin::Module("mod1".into()),
            )
            .unwrap_err();
        assert!(err.to_string().contains("conflict"), "got: {err}");
    }

    #[test]
    fn test_resolver_path_dedup_identical() {
        let mut r = DependencyResolver::new(ConflictStrategy::Error);
        r.add(
            "mylib".into(),
            DepSpec::parse(r#"{ path = "../foo" }"#).unwrap(),
            DepOrigin::Core,
        )
        .unwrap();
        r.add(
            "mylib".into(),
            DepSpec::parse(r#"{ path = "../foo" }"#).unwrap(),
            DepOrigin::Module("mod1".into()),
        )
        .unwrap();
        assert_eq!(r.resolved().len(), 1);
    }

    #[test]
    fn test_resolver_deterministic_order() {
        let mut r = DependencyResolver::new(ConflictStrategy::Error);
        r.add("zlib".into(), DepSpec::Version("1.0".into()), DepOrigin::Template).unwrap();
        r.add("anyhow".into(), DepSpec::Version("1.0".into()), DepOrigin::Template).unwrap();
        r.add("serde".into(), DepSpec::Version("1.0".into()), DepOrigin::Template).unwrap();
        let resolved = r.resolved();
        let names: Vec<&str> = resolved.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["anyhow", "serde", "zlib"]);
    }

    #[test]
    fn test_dep_spec_to_toml_value() {
        let simple = DepSpec::Version("1.0".into());
        assert_eq!(simple.to_toml_value(), toml::Value::String("1.0".into()));

        let full = DepSpec::parse(r#"{ version = "1.0", features = ["derive"] }"#).unwrap();
        let val = full.to_toml_value();
        assert!(val.is_table());
    }
}
