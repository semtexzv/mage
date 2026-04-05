# Distribution and Binary Management

How mage is installed, updated, and resolved at runtime.


## Directory Layout

~/.mage/
  bin/
    mage-gentle-fox              generation 0 (package manager install)
    mage-gentle-fox.meta         metadata sidecar
    mage-happy-wolf              generation 1 (agent-compiled)
    mage-happy-wolf.meta
    mage-brave-eagle             generation 2 (agent-compiled)
    mage-brave-eagle.meta
    generations.jsonl             append-only log: one JSON object per line
  target-cache/                  shared cargo target directory
  extensions/                    system-wide extensions
  sessions/                      session state

/usr/local/bin/mage              wrapper script (installed by package manager)


## The Wrapper Script

Installed to PATH by the package manager. This is the only file the package
manager owns besides the initial binary. Everything else lives in ~/.mage/.

  #!/bin/sh
  set -e
  MAGE_HOME="${MAGE_HOME:-$HOME/.mage}"
  MAGE_GEN="$MAGE_HOME/bin/generations.jsonl"
      echo "mage: not initialized. run the installer again." >&2
      exit 127
  fi

  # --use <petname> selects a specific version
  if [ "$1" = "--use" ] && [ -n "$2" ]; then
      MAGE_NAME="mage-$2"
      shift 2
  else
      # Current generation is the last line. No jq needed.
      MAGE_NAME=$(tail -1 "$MAGE_GEN" | sed 's/.*"name":"\([^"]*\)".*/\1/')
  fi

  MAGE_BIN="$MAGE_HOME/bin/$MAGE_NAME"
  if [ ! -x "$MAGE_BIN" ]; then
      echo "mage: binary not found: $MAGE_BIN" >&2
      echo "mage: available versions:" >&2
      ls "$MAGE_HOME/bin/" | grep '^mage-' | grep -v '\.meta$' | sed 's/^mage-/  /' >&2
      exit 127
  fi

  exec "$MAGE_BIN" "$@"
Properties:
- exec replaces the shell process. User sees one process.
- MAGE_HOME is overridable for non-standard locations.
- Exit 127 matches "command not found" convention.
- The wrapper never changes after install. All updates happen inside ~/.mage/.
- No dependency on jq. Just tail + sed.
- --use allows running any installed version without changing the default.



## generations.jsonl

The single source of truth for which binary is active. Append-only JSONL
(one JSON object per line). The last line is the current generation.

  {"name":"mage-gentle-fox","sdk_version":"0.3.2","sdk_hash":"sha256:a0b1c2...","bundle_hash":"sha256:a1b2c3...","parent":null,"timestamp":"2026-02-10T14:30:00Z","generation":0,"toolchain":"rustc 1.85.0","status":"healthy"}
  {"name":"mage-happy-wolf","sdk_version":"0.3.2","sdk_hash":"sha256:a0b1c2...","bundle_hash":"sha256:d4e5f6...","parent":"mage-gentle-fox","timestamp":"2026-02-12T09:15:00Z","generation":1,"toolchain":"rustc 1.85.0","status":"healthy"}
  {"name":"mage-brave-eagle","sdk_version":"0.3.2","sdk_hash":"sha256:a0b1c2...","bundle_hash":"sha256:g7h8i9...","parent":"mage-happy-wolf","timestamp":"2026-02-13T16:42:00Z","generation":2,"toolchain":"rustc 1.85.0","status":"healthy"}

Properties:

  Append-only. Adding a new generation is appending one line. No rewriting.

  Current = last line. Reading it is tail -1. Trivial in shell, trivial in Rust.

  Rollback = append a new line pointing to an old binary. The log records the
  rollback as an event, not a mutation. History is never lost.

  Status field tracks health check state:
    "healthy"    passed health check (or initial install, assumed healthy)
    "pending"    just compiled, not yet health-checked
    "failed"     failed health check, rolled back from

  On rollback, the monitor appends a new line for the rolled-back-to binary
  with status "healthy" (since it was previously known healthy). The failed
  entry stays in the log with status "failed". Full audit trail.

Example: upgrade attempt that fails and rolls back:

  {"name":"mage-gentle-fox","sdk_version":"0.3.2",...,"status":"healthy"}
  {"name":"mage-happy-wolf",...,"status":"healthy"}
  {"name":"mage-brave-eagle",...,"status":"pending"}
  {"name":"mage-brave-eagle",...,"status":"failed"}
  {"name":"mage-happy-wolf",...,"status":"healthy"}

Last line is mage-happy-wolf (the rollback target). The wrapper script
picks it up. The failed attempt is recorded but not active.

Atomic append: write the line to a temp file, then rename to append.
Or: open with O_APPEND, write is atomic for lines < PIPE_BUF (4096 bytes).
Generation entries are well under that limit.


## Install Methods


### Package manager install

  brew install mage

The package installs two things:
  1. /usr/local/bin/mage          the wrapper script
  2. ~/.mage/bin/mage-{petname}   the actual compiled binary

The package manager's post-install script:
  - Creates ~/.mage/bin/ if needed
  - Copies the binary to ~/.mage/bin/mage-{petname}
  - Writes initial line to generations.jsonl

There is no "mage bootstrap" command. The package manager IS the bootstrap.
The binary it installs is a real compiled mage binary, not a bootstrapper
that downloads another binary. What you install is what you run.


### Snapshot rebuild

Given a snapshot archive from another binary or another machine:

  mage build --from-snapshot snapshot.tar.zst

This:
  1. Extracts the archive to a staging directory
  2. Compiles the binary
  3. Places it at ~/.mage/bin/mage-{petname}
  4. Writes .meta sidecar
  5. Appends to generations.jsonl with status "healthy"

This is how you move mage to a new machine: extract snapshot, build, done.


### Agent self-replication

At runtime, when the agent compiles a new version of itself:

  1. mage-build compiles new binary to ~/.mage/bin/mage-{petname}
  2. Agent writes .meta sidecar
  3. Agent appends to generations.jsonl with status "pending"
  4. Agent writes new binary path to monitor pipe
  5. Agent exits 42
  6. Monitor reads pipe, spawns new binary
  7. Monitor runs health check (see DESIGN-MONITOR.md)
  8. On health check pass: monitor updates status to "healthy"
  9. On health check fail: monitor rolls back "current" to previous, status "failed"

The "pending" status is key. If the monitor crashes between steps 6 and 8,
the next mage invocation sees a "pending" entry as the last line. It can
either try it again or fall back to the last "healthy" entry.


### Manual / cargo install

For developers working on mage itself:
  cd ~/mage
  cargo xtask bootstrap    # uses mage-build to synthesize + install a dev binary


## Binary Naming

Each binary in ~/.mage/bin/ is named:

  mage-{petname}

Examples:
  mage-gentle-fox
  mage-happy-wolf
  mage-brave-eagle

Names are generated by the petname crate (two words, adjective-noun).
If a name collides with an existing binary, regenerate until unique.

Hashes and metadata live in the .meta sidecar and in generations.jsonl.
Not in filenames. Filenames are for humans.


## The .meta Sidecar

Each binary has a companion .meta file:

  {
    "sdk_version": "0.3.2",
    "sdk_hash": "sha256:a0b1c2d3e4f5...",
    "bundle_hash": "sha256:a1b2c3d4e5f6...",
    "parent_hash": "sha256:previous-hash-or-null",
    "parent_name": "mage-happy-wolf",
    "timestamp": "2026-02-13T16:42:00Z",
    "generation": 2,
    "toolchain": "rustc 1.85.0 (abc123 2025-02-01)"
  }

The .meta file is written by mage-build at compile time, alongside the binary.
It duplicates some info from generations.jsonl -- the .meta file is per-binary
and survives even if generations.jsonl is truncated or lost. The embedded snapshot
in the binary itself is the ultimate source of truth; .meta is for quick
access without decompressing the snapshot.


## Cleanup
Old generation binaries accumulate in ~/.mage/bin/.

Commands:

  mage clean                     default: delete target-cache only
  mage clean --generations       delete non-current binaries and their .meta files
  mage clean --all               both: target-cache + old generations

Default is target-cache because it's the most ephemeral data: downloaded
crate artifacts, compiled dependencies. It can be fully rebuilt from scratch.
Deleting it costs compile time on next build but loses nothing.

Generation binaries are more valuable (each is a unique compiled agent with
a specific tool set), so deleting them requires an explicit flag.
Automatic (by monitor):
  After a successful health check of generation N, the monitor can delete
  generation N-2 and older. Keep current (N) and rollback target (N-1).
  Their .meta sidecars are deleted alongside.
  Old lines in generations.jsonl can be compacted (rewrite without old entries).
Running binary deletion (Unix):
  If the monitor is running from an old binary that gets deleted, this is
  fine. Unix unlink() removes the name but the kernel keeps the inode alive
  as long as the process has it mapped. The monitor keeps running. When it
  eventually exits, the disk space is freed.


## Version Selection

Three ways to control which version runs:


### mage versions

List all installed versions:

  mage versions
Output:
  gentle-fox     2026-02-10 14:30  gen 0  sdk 0.3.2  healthy   (initial)
  happy-wolf     2026-02-12 09:15  gen 1  sdk 0.3.2  healthy
  brave-eagle    2026-02-13 16:42  gen 2  sdk 0.3.2  healthy    <- current
Information comes from generations.jsonl. Fast, no binary introspection needed.


### mage use (permanent switch)

Change the default version for all future invocations:

  mage use happy-wolf              switch default to a specific generation
  mage rollback                    switch default to previous generation
Implementation: append the target generation as a new line in generations.jsonl.
The wrapper script picks up the change on next invocation.


### mage --use (single session)

Run a specific version for one session without changing the default:

  mage --use happy-wolf --interactive

The wrapper script intercepts --use before exec. The selected version runs
for this invocation only. generations.jsonl is not modified. Next time the
user runs plain "mage", they get the default (last line) version.

Use cases:
  - Testing whether an older version handles a specific task better
  - Running a known-good version while debugging a broken new one
  - Comparing behavior between versions side by side (two terminals)

Multiple versions can run concurrently. Each is an independent process
with its own session. They share ~/.mage/extensions/ and target-cache
but do not interfere with each other.


### Binary self-identification

Each mage binary knows its own identity. At compile time, the generated
main.rs embeds:

  const MAGE_GENERATION_NAME: &str = "mage-brave-eagle";
  const MAGE_BUNDLE_HASH: &str = "sha256:g7h8i9...";
  const MAGE_GENERATION: u32 = 2;
  const MAGE_SDK_VERSION: &str = "0.3.2";

The binary can report this via:

  mage --version
  mage-brave-eagle (gen 2, sdk 0.3.2, hash sha256:g7h8i9, rustc 1.85.0)

  mage --identity
  {"name":"mage-brave-eagle","generation":2,"sdk_version":"0.3.2","bundle_hash":"sha256:g7h8i9...","parent":"mage-happy-wolf"}

The monitor uses --identity to verify which binary it actually spawned
(defense against races where generations.jsonl changes between read and spawn).

The agent uses its own identity to:
  - Read generations.jsonl and find its own entry
  - Determine its parent (for snapshot lineage)
  - Report its version in health check responses
  - Know which generation it is when writing new entries


## SDK Updates

The mage SDK is published to crates.io. Each compiled binary records which SDK
version it was built against (MAGE_SDK_VERSION). Managing the SDK version is
done through the `mage sdk` subcommand.


### mage sdk current

Shows the SDK version the running binary was built against:

  mage sdk current
  mage sdk 0.3.2 (sha256:a0b1c2...)


### mage sdk upgrade

Checks crates.io for the latest compatible SDK version, rebuilds the binary
against it, and creates a new generation:

  mage sdk upgrade

This is a generation event like any other:
  1. mage-build stages a new build with `mage = "<latest>"` in generated Cargo.toml
  2. Compiles the new binary
  3. Appends to generations.jsonl with status "pending" and the new sdk_version
  4. Hands off to the monitor for health check
  5. On pass: status becomes "healthy". On fail: rolls back to previous generation.

The generation entry records the new sdk_version and sdk_hash, providing full
audit trail of SDK changes in the lineage.

  mage sdk upgrade --to 0.4.0

Upgrades to a specific version instead of latest compatible.


### mage sdk pin

Pins the SDK version so that agent self-replication uses the same version
rather than picking up a newer one:

  mage sdk pin 0.3.2
  mage sdk pin --unpin

When pinned, mage-build injects the pinned version into generated Cargo.toml
regardless of what the latest published version is.


### Automatic update notification

On startup (or periodically during long sessions), the binary checks whether
a newer SDK version is available on crates.io. If one is found, it displays
a non-blocking notification:

  [info] mage sdk 0.4.0 available (current: 0.3.2). Run `mage sdk upgrade` to update.

This is informational only — never an error, never blocks execution. The user
decides when to upgrade. The check is skipped if the SDK version is pinned.

## Windows

Windows strategy:
- Wrapper is mage.cmd that reads last line of generations.jsonl
  and supports --use the same way (parse first arg before dispatching)
- No hardlinks, no symlinks. Just the JSONL file pointing to a filename.
- Running exe cannot be deleted. Old binaries are renamed to .old
  and deleted on next startup.
- generations.jsonl works identically across platforms.
- --identity, --version work the same.


## Summary

Component              Location                   Managed by           Lifetime
Wrapper script         /usr/local/bin/mage        Package manager      Until uninstalled
generations.jsonl      ~/.mage/bin/               Agent / monitor      Appended on each new version
Generation binaries    ~/.mage/bin/mage-*         mage-build           Until cleaned up
Meta sidecars          ~/.mage/bin/mage-*.meta    mage-build           Alongside their binary
Target cache           ~/.mage/target-cache/      mage-build           Until mage clean
