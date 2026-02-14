# metarust

Dynamic Rust workspace compiler for AI agents.

## Overview

metarust takes a set of Rust module files with `// @dep` annotations,
a Jinja2 entry-point template, and assembles + compiles a complete
Cargo workspace. Designed for AI agents that generate Rust code and
need rapid compile-test cycles.

## Key Concepts

### Modules

A module is either a single `.rs` file or a `mod.rs`-rooted directory.
Dependencies are declared inline via comments:

```rust
// @dep serde = "1.0"
// @dep serde_json = "1.0"
// @dep my_other_module
```

- `// @dep crate = "version"` adds an external crate dependency.
- `// @dep module_name` declares an internal cross-module dependency.

Modules are discovered from configured module root directories or
specified as direct file paths.

### Templates

Jinja2 templates that produce the `main.rs` entry point and optionally
extra Cargo dependencies. Templates receive the list of resolved
modules as context, allowing conditional code generation based on which
modules are included.

### Bundle

A Bundle combines modules, a template, a toolchain, and optional
shared libraries into a generated Cargo workspace under
`~/.mr/workspaces/{id}/`. The workspace references module source files
at their original paths via absolute `#[path]` attributes -- no files
are copied.

### Toolchain

The Rust toolchain is auto-detected from the system PATH, or can be
specified via sysroot or explicit `cargo`/`rustc` paths. A downloader
exists for fetching specific Rust toolchain versions, but this is not
considered stable.

## CLI Usage

### Verify toolchain

```sh
metarust init
```

Resolves and prints the detected system toolchain (cargo, rustc,
version, host, sysroot).

### Build

```sh
metarust build \
  --id my-app \
  --template path/to/main.rs.j2 \
  --deps-template path/to/deps.j2 \
  --module path/to/foo.rs \
  --module bar \
  --modroot ./modules/ \
  --shared-libs ./libs/common \
  --target x86_64-unknown-linux-gnu \
  --approot /tmp/mr
```

Key flags:

| Flag               | Description                                       |
| ------------------ | ------------------------------------------------- |
| `--id`             | Name/ID for the generated workspace               |
| `--template`       | Jinja2 template for `main.rs`                     |
| `--deps-template`  | Optional Jinja2 template for extra Cargo deps     |
| `-m, --module`     | Module to include (path or name to resolve)        |
| `-M, --modroot`    | Directory to search for named modules              |
| `--shared-libs`    | Shared library dirs to include in the workspace    |
| `--approot`        | Root directory for workspaces (default: `~/.mr`)   |
| `--target`         | Target triple for cross-compilation                |
| `--sysroot`        | Path to a custom Rust toolchain sysroot            |
| `--cargo-path`     | Path to a specific `cargo` executable              |
| `--rustc-path`     | Path to a specific `rustc` executable              |

## Library Usage

```rust
use metarust::bundle::Bundle;
use metarust::module::Module;

let module = Module::parse_file("path/to/foo.rs", "foo")?;

let bundle = Bundle::new("my-app")
    .with_template(my_template)
    .add_module(module);

bundle.generate()?;
let result = bundle.compile()?;

if result.success {
    println!(
        "Binary: {}",
        result.executable_path.unwrap().display()
    );
}
```

`with_template` accepts any type implementing the `Template` trait.
Provide your own implementation or use the Jinja2-based `FileTemplate`
from the CLI as a reference.

## Cargo Features

| Feature | Default | Description                                    |
| ------- | ------- | ---------------------------------------------- |
| `tokio` | Yes     | Async runtime; uses `reqwest` for downloads    |
| `ureq`  | No      | Synchronous HTTP backend (alt. to reqwest)     |
| `actix` | No      | Actix-based async HTTP backend                 |

These features control which HTTP backend is used by the toolchain
downloader.

## Project Structure

```
src/
  lib.rs        -- Public API root, default_approot()
  main.rs       -- CLI entry point (clap)
  bundle.rs     -- Bundle, Config, Template trait, workspace gen
  compile.rs    -- CompilationResult, diagnostics
  module.rs     -- Module parsing, dep extraction, resolution
  toolchain.rs  -- Toolchain detection and metadata
  downloader.rs -- Toolchain download backends
  error.rs      -- Error types
```
