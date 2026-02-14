# Reproducible Builds

How mage produces byte-identical binaries across machines.


## Path Embedding in Rust Binaries

Rust embeds filesystem paths into binaries through these mechanisms:

  Mechanism                           Where it appears                  Remappable?
  file!() macro                       panic messages, error reporting   Yes (--remap-path-prefix)
  #[track_caller] / Location          panic messages                    Yes
  panic!() location                   runtime panic output              Yes
  module_path!()                      compiled into binary              No (crate name, not fs path)
  env!("CARGO_MANIFEST_DIR")          compiled into binary              NO -- string literal from env var
  DWARF debug info (DW_AT_comp_dir)   debug symbols                     Yes
  N_OSO stab entries (macOS)          object file refs in debug builds  No (linker, not rustc)


## What Cargo Already Does Right

Cargo passes relative paths to rustc. When you run cargo build, the compiler
receives src/main.rs, not /home/alice/projects/myapp/src/main.rs. This means:

  file!()              returns src/main.rs (relative to crate root)
  Location::caller()   returns src/main.rs
  panic!()             reports src/main.rs:42:5
  module_path!()       returns myapp::config (crate name, no filesystem path)

These are already stable across machines as long as the workspace layout is
identical -- which mage-build guarantees by generating a deterministic workspace.

Exception: path dependencies outside the workspace. When a crate is referenced
via path = "../somewhere/else" and lives outside the workspace root, Cargo
passes its absolute path to rustc. mage-build avoids this by keeping everything
inside the generated workspace via symlinks.


## The Bundle Hash

The bundle hash is the single identity that drives everything downstream:

  Staging directory path:   /tmp/mage/{bundle-hash}/
  Cache key:                same hash = same binary already compiled, skip build
  Snapshot identity:        embedded in mage-snapshot.json as bundle_hash
  Petname seed:             hash bytes seed the petname generator
  Reproducibility:          same hash = same inputs = same binary

The hash is a SHA-256 digest of content only. No paths, no timestamps, no
machine-specific information.


### What goes into the hash

  Input                                How it's hashed
  Bundle ID                            id.as_bytes()
  Module sources                       sorted by name, then for each: name + file content
  Template output                      rendered main.rs source + sorted dependency strings
  SDK version                          mage crate version string (e.g. "0.3.2")
  SDK registry checksum                checksum from crates.io index
  Patch configuration                  sorted by (registry, crate), then serialized patch spec
  Asset content                        sorted by path, then for each: path + content bytes
  Toolchain VERSION STRING             e.g. "rustc 1.85.0 (abc123def 2025-02-01)"
  Target triple                        e.g. "aarch64-apple-darwin" or "native"
  Resolved dependency versions         from Cargo.lock: sorted (name, version, checksum) triples

Pseudocode:

  fn content_hash(bundle: &Bundle) -> String {
      let mut h = Sha256::new();

      h.update(bundle.id.as_bytes());

      let mut modules: Vec<_> = bundle.modules.iter().collect();
      modules.sort_by_key(|m| &m.name);
      for m in &modules {
          h.update(m.name.as_bytes());
          h.update(fs::read_to_string(&m.path)?.as_bytes());
      }

      if let Some(ref template) = bundle.template {
          let ctx = RenderContext { modules: &bundle.modules };
          h.update(template.render_main(&ctx)?.as_bytes());
          let mut deps: Vec<String> = template.render_dependencies(&ctx)?
              .iter().map(|d| format!("{d:?}")).collect();
          deps.sort();
          for d in &deps { h.update(d.as_bytes()); }
      }

      // SDK: version + registry checksum (not source, not path)
      h.update(bundle.sdk_version.as_bytes());  // e.g. "0.3.2"
      if let Some(ref checksum) = bundle.sdk_checksum {
          h.update(checksum.as_bytes());  // from crates.io index
      }

      // Toolchain: version string only, NOT paths
      if let Some(ref tc) = bundle.toolchain {
          h.update(tc.version_string.as_bytes());  // "rustc 1.85.0 (abc123...)"
          if let Some(ref target) = tc.target {
              h.update(target.as_bytes());
          }
      }

      let mut assets: Vec<_> = bundle.assets.keys().collect();
      assets.sort();
      for key in &assets {
          h.update(key.to_string_lossy().as_bytes());
          h.update(&bundle.assets[key]);
      }

      // Resolved dependency versions from Cargo.lock
      // This catches cargo update changing transitive dep versions
      // even when the declared deps (in @dep annotations) haven't changed.
      if let Some(ref lockfile) = bundle.cargo_lock {
          let mut packages: Vec<_> = lockfile.packages.iter()
              .map(|p| (&p.name, &p.version, p.checksum.as_deref().unwrap_or("")))
              .collect();
          packages.sort();
          for (name, version, checksum) in &packages {
              h.update(name.as_bytes());
              h.update(version.as_bytes());
              h.update(checksum.as_bytes());
          }
      }

      format!("{:x}", h.finalize())
  }


### What does NOT go into the hash

  Excluded                     Why
  Absolute paths               Machine-specific. /Users/alice vs /home/bob
  Toolchain binary paths       Machine-specific. Use version string instead.
  Approot / home directory     Machine-specific
  Hostname                     Machine-specific
  Timestamps                   Non-deterministic
  CARGO_MANIFEST_DIR value     Derived from staging path, which is derived from hash
  SDK source code              Covered by version + registry checksum. Same version = same code.


### Why toolchain version string, not paths

The current metarust content_hash() hashes tc.rustc_path and tc.cargo_path.
These are absolute paths like /Users/alice/.rustup/toolchains/stable/bin/rustc.
This means two machines with identical source code but different rustup install
locations produce different hashes, different staging dirs, and different binaries.

Fix: hash the toolchain version string ("rustc 1.85.0 (abc123def 2025-02-01)")
instead. Same compiler version = same hash. Different versions = different hash.
This is the correct granularity -- we want different binaries for different
compilers, but not for different install locations.

The version string is obtained from rustc --version --verbose (already parsed
by the Toolchain struct).


## Fixed Staging Root

The staging directory must be at a fixed, well-known path so that
env!("CARGO_MANIFEST_DIR") expands to the same value on every machine.

  Unix:     /tmp/mage/{bundle-hash}/
  Windows:  C:\mage\{bundle-hash}\

NOT std::env::temp_dir() because:
  - macOS: $TMPDIR is /var/folders/xx/.../T/ (random per-user)
  - Linux: /tmp (stable, but let's be explicit)
  - Windows: C:\Users\alice\AppData\Local\Temp (user-specific)

Hardcoded to /tmp/mage/ on Unix. Override with MAGE_BUILD_ROOT env var.

This means env!("CARGO_MANIFEST_DIR") expands to:
  /tmp/mage/a1b2c3d4e5f6.../

Two machines with the same bundle hash get the same CARGO_MANIFEST_DIR value.
Combined with --remap-path-prefix, the binary contains no machine-specific paths.


## Symlinks: Why Not Copies

Extension module sources are symlinked into the staging directory, not copied.
Core crates (the SDK) are no longer in the staging directory — they are fetched
from the crates.io registry by Cargo as normal dependencies. Symlinks apply only
to extension module sources. This matters for two reasons:

1. Mtime preservation for Cargo fingerprinting

   Cargo's crate-level cache uses file mtimes to detect changes. When you
   symlink a file, Cargo follows the symlink and reads the real file's mtime.

   - Edit the real source file -> Cargo detects change -> recompiles that crate
   - Don't edit -> Cargo skips recompilation (cache hit)
   - No mtime pollution from copying

   Copying creates fresh mtimes on every build, forcing full recompilation of
   every crate every time. With symlinks, only changed crates recompile.
   For a workspace with tokio, serde, reqwest etc., this saves minutes.

2. Disk space

   Extension module sources may be shared across builds. Symlinks cost nothing.
   Copies duplicate them per staging directory.

Caveat: Cargo calls canonicalize() on the workspace root. Symlinks to the
workspace root itself don't provide path stability. But symlinks WITHIN the
workspace (modules/) work fine because Cargo sees them as relative path
dependencies. Core crates (the SDK) are fetched from the registry by Cargo
and never appear as symlinks in the staging directory.

The staging directory itself is a real directory (not a symlink) at the
fixed path /tmp/mage/{bundle-hash}/.


## The Recipe

### Release builds (default)

  CARGO_INCREMENTAL=0
  RUSTFLAGS="--remap-path-prefix /tmp/mage/{hash}=/build"
  cargo build --release

Two env vars. CARGO_TARGET_DIR is optional for release (stabs are stripped).

### Debug builds

  CARGO_INCREMENTAL=0
  CARGO_TARGET_DIR=~/.mage/target-cache
  RUSTFLAGS="--remap-path-prefix /tmp/mage/{hash}=/build"
  cargo build

Three env vars. CARGO_TARGET_DIR is required to stabilize N_OSO stab entries
on macOS (the target/ path appears in debug symbols).

### In mage-build's spawn_cargo

  Command::new(&toolchain.cargo_path)
      .args(&["build", "--message-format=json", "--release"])
      .current_dir(&staging_dir)
      .env("RUSTC", &toolchain.rustc_path)
      .env("CARGO", &toolchain.cargo_path)
      .env("CARGO_INCREMENTAL", "0")
      .env("RUSTFLAGS", format!(
          "--remap-path-prefix {}=/build",
          staging_dir.display()
      ))
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .spawn()


## CARGO_INCREMENTAL=0: What It Actually Does

Disables intra-crate incremental compilation. Does NOT disable Cargo's
crate-level fingerprint caching. These are separate systems:

  System                    What it caches                              Controlled by
  Cargo fingerprinting      Whole crate artifacts (if serde didn't      Always on
                            change, skip it)
  Incremental compilation   Intra-crate codegen units (within one       CARGO_INCREMENTAL
                            crate, reuse unchanged functions)

With CARGO_INCREMENTAL=0, after touching main.rs:
  - serde, syn, tokio, etc: NOT recompiled (cached by Cargo fingerprints)
  - Your application crate: fully recompiled (instead of just the changed CGU)

The performance cost is negligible for small-to-medium crates. The
reproducibility gain is essential: incremental compilation embeds random
session IDs in .rcgu.o filenames, which leak into macOS N_OSO stab entries.


## env!("CARGO_MANIFEST_DIR"): The One Unremappable Leak

env!("CARGO_MANIFEST_DIR") expands to the absolute, canonicalized path of the
directory containing Cargo.toml. It is:

  - NOT affected by --remap-path-prefix (it's a string literal from an env var)
  - NOT affected by symlinks (Cargo canonicalizes first)
  - Embedded in the binary if any code uses it

Mitigation:

  1. Fixed staging root: /tmp/mage/{bundle-hash}/ is identical across machines
     with the same bundle hash, so CARGO_MANIFEST_DIR is already stable.

  2. Audit dependencies. Common uses:
     - include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/path"))
       -> replace with include_str!("relative/path")
     - Build scripts using CARGO_MANIFEST_DIR for asset paths
       -> use relative paths or OUT_DIR instead

  3. If a dependency uses env!("CARGO_MANIFEST_DIR"), the fixed staging root
     ensures the value is stable. The path /tmp/mage/{hash}/ may still appear
     in the binary via strings, but it's deterministic.

  4. --remap-path-prefix does NOT help here. Accept this as a known limitation.
     The value is deterministic (thanks to fixed root), just not "clean".


## Verification

To verify reproducibility, build the same bundle on two machines and compare:

  shasum -a 256 binary_a binary_b

To check for path leaks:

  strings /path/to/binary | grep -i "/home/\|/Users/\|/tmp/"

Expected output with fixed staging root: only /tmp/mage/{hash}/ references
(from CARGO_MANIFEST_DIR if any dep uses it). No /Users/alice or /home/bob.


## What Is NOT Covered

  - Timestamps in PE/COFF headers (Windows): use RUSTFLAGS="-C link-arg=/Brepro"
  - Build script output (build.rs): if a build script embeds paths via
    cargo:rustc-env, those are not remapped. Audit build scripts.
  - Proc macro crates that read CARGO_MANIFEST_DIR at compile time: audit deps
  - Different compiler versions: expected to produce different binaries
  - Different target triples: obviously different binaries
