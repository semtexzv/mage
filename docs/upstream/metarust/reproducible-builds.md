# Reproducible Builds in Metarust

How Rust embeds filesystem paths into compiled binaries, and how metarust neutralizes
them to produce byte-identical artifacts across machines.

---

## The Problem

Rust embeds filesystem paths into binaries through several mechanisms:

| Mechanism | Where it appears | Example |
|---|---|---|
| `file!()` macro | Source code, compiled into binary | `src/main.rs` |
| `#[track_caller]` / `Location::caller()` | Panic messages, error reporting | `src/server.rs:12:2` |
| `panic!()` location | Runtime panic output | `thread 'main' panicked at src/lib.rs:42` |
| `module_path!()` | Compiled into binary | `myapp::config` |
| `env!("CARGO_MANIFEST_DIR")` | Compiled into binary as string literal | `/home/alice/projects/myapp` |
| DWARF debug info (`DW_AT_comp_dir`) | Debug symbols | `/rustc/57d2fb136...` |
| Mach-O N_OSO stab entries (macOS) | Object file references in debug builds | `/path/to/target/debug/deps/foo.rcgu.o` |

If two developers build the same code from different directories, the binaries can
differ solely because of these embedded paths.

## What Cargo Already Does Right

Cargo passes **relative paths** to `rustc`. When you run `cargo build`, the compiler
receives `src/main.rs`, not `/home/alice/projects/myapp/src/main.rs`. This means:

- **`file!()`** returns `src/main.rs` (relative to crate root)
- **`Location::caller()`** returns `src/main.rs` (same)
- **`panic!()`** reports `src/main.rs:42:5` (same)
- **`module_path!()`** returns `myapp::config` (crate name, no filesystem path)

These are already stable across machines **as long as the workspace layout is
identical** — which metarust guarantees by generating a deterministic workspace
structure.

### Exception: Path Dependencies Outside the Workspace

When a crate is referenced via `path = "../somewhere/else"` and lives outside the
workspace root, Cargo passes its **absolute path** to rustc. This breaks
reproducibility in two ways:

1. `file!()` in that crate returns an absolute path
2. Cargo's metadata hash (`-C metadata`) incorporates the absolute source path,
   producing different symbol hashes

**Metarust avoids this by keeping everything inside the generated workspace.** Modules,
shared libraries, and core crates are symlinked into the workspace tree, so all paths
are workspace-relative.

## What Requires Explicit Handling

### 1. `--remap-path-prefix` (Stable since Rust 1.51)

Rewrites path prefixes in all compiler-emitted paths: `file!()`, panic locations,
debug info. This is the primary tool for reproducibility.

```
--remap-path-prefix /real/absolute/path=/stable/prefix
```

Multiple prefixes can be specified:

```
--remap-path-prefix /path/to/workspace=/BUILD \
--remap-path-prefix /path/to/libs=/LIBS
```

Each prefix is matched independently. The longest matching prefix wins.

**What it covers:** `file!()`, `Location::caller()`, panic locations, DWARF debug info
paths.

**What it does NOT cover:** `env!("CARGO_MANIFEST_DIR")` — this is a string literal
injected by Cargo as an environment variable, not a path in the compiler pipeline.
`--remap-path-prefix` operates on rustc's internal path representation, not on
arbitrary string values from `env!()`.

### 2. `CARGO_INCREMENTAL=0`

Incremental compilation generates a **random session ID** per invocation. This ID
appears in `.rcgu.o` filenames:

```
myapp.0yc9nqsu66c79sgpl5gac122k.1m3or18.rcgu.o   <- session "1m3or18"
myapp.0yc9nqsu66c79sgpl5gac122k.0gf7274.rcgu.o   <- session "0gf7274"
                                 ^^^^^^^
                                 random per invocation
```

On macOS, the linker copies these filenames into N_OSO stab entries in the binary.
`--remap-path-prefix` cannot fix this because it operates at the rustc level, before
the linker runs.

**Impact on caching:** `CARGO_INCREMENTAL=0` disables only **intra-crate** incremental
compilation (CGU-level reuse within a single crate). It does NOT disable Cargo's
**crate-level** caching (the fingerprint system). These are two separate systems:

| System | What it caches | Controlled by |
|---|---|---|
| Cargo fingerprinting | Whole crate artifacts — if `serde` didn't change, skip it | Always on, cannot be disabled |
| Incremental compilation | Intra-crate codegen units — within one crate, reuse unchanged functions | `CARGO_INCREMENTAL` |

With `CARGO_INCREMENTAL=0`, after touching `main.rs`:
- `serde`, `syn`, `tokio`, etc. → **not recompiled** (cached by Cargo fingerprints)
- Your application crate → **fully recompiled** (instead of just the changed CGU)

For small-to-medium crates, the cost is negligible.

### 3. `CARGO_TARGET_DIR` (Debug Builds Only)

The `target/` directory path appears in N_OSO stab entries on macOS. Setting a fixed
`CARGO_TARGET_DIR` ensures these paths are identical across machines.

For **release builds**, this is unnecessary — stabs are stripped.

## Symlinks: Behavior and Implications

### Cargo Resolves Symlinks

Cargo calls `canonicalize()` on the project directory. If you `cd /tmp/link_to_project
&& cargo build`, Cargo resolves the symlink and operates on the real path. Symlinks to
the project root **do not** provide a stable path.

### rustc Does NOT Resolve Symlinks

`rustc` uses the path **exactly as given on the command line**. Since Cargo always
passes relative paths (`src/main.rs`), symlinks in the source tree are invisible to
the compiler output.

### Symlinks Preserve mtime for Cargo Fingerprinting

When source files are symlinked into the workspace, Cargo follows the symlink and
reads the **real file's mtime** for fingerprinting. This means:

- Edit the real source file → Cargo detects the change → recompiles that crate
- Don't edit → Cargo skips recompilation
- No mtime pollution from copying files

This is critical for metarust: modules, shared libraries, and core crates should be
**symlinked** into the generated workspace, not copied. Copying creates fresh mtimes
on every generation, forcing unnecessary recompilation.

## `env!("CARGO_MANIFEST_DIR")` — The Unrempable Leak

`env!("CARGO_MANIFEST_DIR")` expands to the absolute, canonicalized path of the
directory containing `Cargo.toml`. It is:

- **Not affected by `--remap-path-prefix`** (it's a string literal from an env var)
- **Not affected by symlinks** (Cargo canonicalizes first)
- **Embedded in the binary** if any code uses it

**Mitigation:** Audit your code and dependencies. Common uses:

- `include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/path"))` — replace with
  `include_str!("relative/path")` which works relative to the source file
- Build scripts using `CARGO_MANIFEST_DIR` to find assets — use relative paths or
  `OUT_DIR` instead

If a dependency uses `env!("CARGO_MANIFEST_DIR")`, the path leaks. In metarust's
case, the workspace is generated at `approot/workspaces/{id}`, so if `approot` is
consistent across machines (e.g., `~/.mr`), the `CARGO_MANIFEST_DIR` value is already
stable.

## Split Debug Info

Three modes, all compatible with `--remap-path-prefix`:

| Mode | Flag | Behavior | Binary contains debug info? |
|---|---|---|---|
| `off` | `-C split-debuginfo=off` | DWARF embedded in binary | Yes |
| `packed` | `-C split-debuginfo=packed` | `.dSYM` bundle (macOS) / `.dwp` (Linux) | No, separate file |
| `unpacked` | `-C split-debuginfo=unpacked` | Individual `.o` files with stabs | No, separate files |

All three modes produce **byte-identical release binaries** when combined with
`--remap-path-prefix`, `CARGO_INCREMENTAL=0`, and `CARGO_TARGET_DIR`.

For release builds with debug info, use `packed`: the binary is clean and reproducible,
and the `.dSYM`/`.dwp` can be archived separately for debugging.

```toml
[profile.release]
debug = 2                    # full debug info
split-debuginfo = "packed"   # separate .dSYM / .dwp file
```

## The Recipe

### Release Builds

```sh
CARGO_INCREMENTAL=0 \
RUSTFLAGS="--remap-path-prefix $(realpath $WORKSPACE_DIR)=/BUILD" \
cargo build --release
```

Two flags. `CARGO_TARGET_DIR` is optional for release (stabs are stripped).

### Debug Builds

```sh
CARGO_INCREMENTAL=0 \
CARGO_TARGET_DIR=/path/to/fixed/target \
RUSTFLAGS="--remap-path-prefix $(realpath $WORKSPACE_DIR)=/BUILD" \
cargo build
```

Three flags. `CARGO_TARGET_DIR` is required to stabilize N_OSO stab entries on macOS.

### In Metarust's `spawn_cargo`

```rust
fn spawn_cargo(toolchain: &Toolchain, workspace_dir: &Path) -> Result<Child> {
    let canonical = workspace_dir.canonicalize()?;
    let remap_flag = format!(
        "--remap-path-prefix {}=/BUILD",
        canonical.display()
    );

    Command::new(&toolchain.cargo_path)
        .args(&["build", "--message-format=json", "--release"])
        .current_dir(workspace_dir)
        .env("RUSTC", &toolchain.rustc_path)
        .env("CARGO", &toolchain.cargo_path)
        .env("CARGO_INCREMENTAL", "0")
        .env("RUSTFLAGS", &remap_flag)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Compilation(format!("spawn failed: {e}")))
}
```

## Workspace Layout Requirements

For reproducible builds, the workspace layout must be identical across machines.
Metarust achieves this by generating a deterministic structure:

```
approot/workspaces/{id}/
  Cargo.toml
  src/
    main.rs          (generated from template)
  libs/
    shared_lib_a/    (symlink to real shared lib)
    shared_lib_b/    (symlink to real shared lib)
  modules/
    module_a.rs      (symlink to real module file)
    module_b/        (symlink to real module dir)
  core/
    core_crate/      (symlink to real core crate)
```

**Key rules:**

1. All path dependencies must be **inside** the workspace tree (use symlinks)
2. Path dependencies outside the workspace produce different Cargo metadata hashes
   across machines, breaking binary identity even if `file!()` is remapped
3. Symlinks preserve real file mtimes, so Cargo's crate-level caching works correctly
4. Module source changes trigger recompilation of only the affected crate

## Verification

To verify reproducibility, build the same workspace from two different directories and
compare SHA-256 hashes:

```sh
shasum -a 256 /path/to/binary_a /path/to/binary_b
```

To check for path leaks in a binary:

```sh
strings /path/to/binary | grep -i "/home/\|/Users/\|/tmp/"
```

## What is NOT Covered

- **Timestamps in PE/COFF headers** (Windows) — use `RUSTFLAGS="-C link-arg=/Brepro"`
- **Build script output** (`build.rs`) — if a build script embeds paths via
  `cargo:rustc-env`, those are not remapped
- **Proc macro crates** that read `CARGO_MANIFEST_DIR` at compile time — audit
  dependencies
- **Different compiler versions** — binaries from different rustc versions are not
  expected to match
- **Different target triples** — obviously produces different binaries
