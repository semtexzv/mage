# Session Persistence Format

**Status:** Implemented
**Crate:** `agent-core`
**Files:** `entry.rs`, `store.rs`, `message.rs` (for `EntryId`)

## 1. Overview

Sessions are persisted as append-only JSONL files. Each line is a self-contained JSON object with a `"type"` discriminator. The first line is always a session header; subsequent lines are entries that form a linked list via `id` / `parent_id` fields.

Format version: **3** (pi-mono v3 compatible).

## 2. File Layout

```
<session_dir>/
  --<encoded_cwd>--/
    <timestamp>_<uuid>.jsonl
```

- `encoded_cwd`: the working directory with `/` replaced by `-`.
- `timestamp`: ISO 8601 with colons replaced by `-` (filesystem-safe).
- `uuid`: v4 UUID identifying the session.

Example: `--Users-alice-project--/2024-01-15T10-30-00.000Z_a1b2c3d4-e5f6-....jsonl`

## 3. Header

The first line of every JSONL file is a `session` entry containing the `SessionHeader`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionHeader {
    pub version: u32,
    pub id: String,
    pub timestamp: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session: Option<String>,
}
```

Serialized example:

```json
{"type":"session","version":3,"id":"a1b2c3d4-...","timestamp":"2024-01-15T10:30:00.000Z","cwd":"/Users/alice/project"}
```

## 4. Entry Types

### 4.1 FileEntry Enum

All lines (including the header) are variants of `FileEntry`, tagged via `#[serde(tag = "type")]`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileEntry {
    #[serde(rename = "session")]
    Session(SessionHeader),
    Message(MessageEntry),
    ModelChange(ModelChangeEntry),
    ThinkingLevelChange(ThinkingLevelEntry),
    Compaction(CompactionEntry),
    BranchSummary(BranchSummaryEntry),
    Custom(CustomEntry),
    CustomMessage(CustomMessageEntry),
    Label(LabelEntry),
    SessionInfo(SessionInfoEntry),
    #[serde(other)]
    Unknown,
}
```

The `#[serde(other)]` on `Unknown` means unrecognized `type` values deserialize as `Unknown` instead of failing, providing forward compatibility.

Serde tagging strategy: **internally tagged** via `"type"` field with `snake_case` variant names (except `Session` which is explicitly renamed to `"session"`).

### 4.2 EntryId

`EntryId` is defined in `message.rs` (not `entry.rs`). It is a transparent newtype over `LocalStr`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(LocalStr);
```

Generated via `EntryId::generate()` which produces the first 8 characters of a v4 UUID, retrying up to 100 times to avoid collisions with existing IDs. Falls back to a full UUID on exhaustion.

### 4.3 Entry Structs

All entry structs share a common shape: `id`, optional `parent_id`, and `timestamp`. All use `#[serde(rename_all = "camelCase")]`.

**MessageEntry** — wraps a conversation `Message`:

```rust
pub struct MessageEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub timestamp: String,
    pub message: Message,
}
```

**ModelChangeEntry** — records a provider/model switch:

```rust
pub struct ModelChangeEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub timestamp: String,
    pub provider: String,
    pub model_id: String,
}
```

**ThinkingLevelEntry** — records a thinking-level change:

```rust
pub struct ThinkingLevelEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub timestamp: String,
    pub thinking_level: ThinkingLevel,
}
```

**CompactionEntry** — marks a compaction point with summary:

```rust
pub struct CompactionEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub timestamp: String,
    pub summary: String,
    pub first_kept_entry_id: EntryId,
    pub tokens_before: u64,
    pub details: Option<Value>,  // skip_serializing_if = "Option::is_none"
}
```

**BranchSummaryEntry** — summarizes a branch the conversation returned from:

```rust
pub struct BranchSummaryEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub timestamp: String,
    pub from_id: EntryId,
    pub summary: String,
    pub details: Option<Value>,  // skip_serializing_if = "Option::is_none"
}
```

**CustomEntry** — extension-defined structured data:

```rust
pub struct CustomEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub timestamp: String,
    pub custom_type: String,
    pub data: Value,
}
```

**CustomMessageEntry** — extension-defined message that can appear in context:

```rust
pub struct CustomMessageEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub timestamp: String,
    pub custom_type: String,
    pub content: UserMessageContent,
    pub display: bool,
    pub details: Option<Value>,  // skip_serializing_if = "Option::is_none"
}
```

**LabelEntry** — attaches or removes a label on a target entry:

```rust
pub struct LabelEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub timestamp: String,
    pub target_id: EntryId,
    pub label: Option<String>,  // skip_serializing_if = "Option::is_none"
}
```

**SessionInfoEntry** — sets the session's display name:

```rust
pub struct SessionInfoEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub timestamp: String,
    pub name: String,
}
```

### 4.4 iso_now() Helper

`entry.rs` exports a public `iso_now()` helper. `store.rs` has a private `iso8601_now()` with the same algorithm. Both produce ISO 8601 UTC timestamps without a `chrono` dependency, using Howard Hinnant's civil calendar algorithm:

```rust
/// Returns current UTC time as an ISO 8601 string (e.g. "2024-01-15T10:30:00.000Z").
pub fn iso_now() -> String
```

## 5. Entry Semantics

Entries fall into two categories:

| Category | Entry Types | Behavior |
|---|---|---|
| **Reified** (appear in context) | `Message`, `CustomMessage`, `BranchSummary` | Converted to `Message` values by `build_context()` |
| **Latest-wins** (settings) | `ModelChange`, `ThinkingLevelChange` | Only the last one on the path applies |
| **Metadata** | `Compaction`, `Label`, `SessionInfo`, `Custom` | Structural/informational; not directly emitted as messages |

## 6. SessionStore

### 6.1 Struct

```rust
pub struct SessionStore {
    header: SessionHeader,
    entries: Vec<FileEntry>,
    by_id: HashMap<EntryId, usize>,
    leaf_id: Option<EntryId>,
    file: Option<PathBuf>,
    dirty: Vec<FileEntry>,
    ready: bool,
}
```

- `entries`: all non-header entries in insertion order.
- `by_id`: index from `EntryId` to position in `entries`.
- `leaf_id`: the current tip of the conversation (most recently appended entry).
- `file`: filesystem path; `None` for in-memory stores.
- `dirty`: entries appended since last flush.
- `ready`: gate for I/O; flush is a no-op until `mark_ready()` is called.

### 6.2 Creation Methods

```rust
pub fn create(cwd: &str, session_dir: Option<&Path>) -> Self
```
Creates a new session with a v4 UUID, version 3 header, and optional file path. The file path is derived as `<session_dir>/--<encoded_cwd>--/<timestamp>_<id>.jsonl`.

```rust
pub fn in_memory(cwd: &str) -> Self
```
Creates a session with no backing file. `file` is `None`, so `flush()` is always a no-op.

```rust
pub fn open(path: &Path) -> io::Result<Self>
```
Reads an existing JSONL file. Parses each line as `FileEntry`, skipping empty lines and unparseable lines. Extracts the header from the `Session` variant. Skips `Unknown` entries. Returns `Err` if no header is found. Sets `ready = true`.

### 6.3 Append Methods

All append methods follow the same pattern: generate a new `EntryId`, set `parent_id` to the current `leaf_id`, set `timestamp` to `iso8601_now()`, push to both `entries` and `dirty`, and update `leaf_id`. All return the new `EntryId`.

```rust
pub fn append_message(&mut self, message: &Message) -> EntryId
```
Appends a `Message` entry. If `message.is_ephemeral()`, returns a generated ID without storing.

```rust
pub fn append_model_change(&mut self, provider: &str, model_id: &str) -> EntryId
pub fn append_thinking_level_change(&mut self, level: llm::ThinkingLevel) -> EntryId
pub fn append_compaction(&mut self, summary: &str, first_kept: &EntryId, tokens: u64) -> EntryId
pub fn append_branch_summary(&mut self, from_id: &EntryId, summary: &str) -> EntryId
pub fn append_custom(&mut self, custom_type: &str, data: Value) -> EntryId
pub fn append_custom_message(&mut self, custom_type: &str, content: &llm::UserMessageContent, display: bool, details: Option<Value>) -> EntryId
pub fn append_label(&mut self, target_id: &EntryId, label: Option<&str>) -> EntryId
pub fn append_session_info(&mut self, name: &str) -> EntryId
```

### 6.4 Tree Navigation

```rust
pub fn leaf_id(&self) -> Option<&EntryId>
```
Returns the current leaf entry ID.

```rust
pub fn set_leaf(&mut self, id: &EntryId)
```
Moves the leaf pointer. Panics if `id` is not in the store.

```rust
pub fn walk_to_root(&self, from: &EntryId) -> Vec<&FileEntry>
```
Follows `parent_id` links from `from` back to the root, returning entries in leaf-to-root order.

### 6.5 Context Building

```rust
pub fn build_context(&self) -> SessionContext
```
See Section 7.

### 6.6 SessionContext

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContext {
    pub messages: Vec<Message>,
    pub thinking_level: llm::ThinkingLevel,
    pub model: Option<(String, String)>,
}
```

`model` is a `(provider, model_id)` tuple. `thinking_level` defaults to `ThinkingLevel::default()`.

## 7. build_context() Algorithm

`build_context()` reconstructs the conversation context from the entry tree:

1. **Get leaf.** If `leaf_id` is `None`, return empty `SessionContext`.

2. **Walk to root.** Call `walk_to_root(leaf)` to get entries in leaf-to-root order, then **reverse** to get root-to-leaf order. This is the *path*.

3. **Extract latest-wins settings.** Iterate the path once:
   - `ModelChange` entries: update `model` to `(provider, model_id)`.
   - `ThinkingLevelChange` entries: update `thinking_level`.
   - `Message` entries with `Assistant` body: also update `model` from the assistant message's `provider` and `model` fields.

4. **Find last compaction.** Scan the path for the last `Compaction` entry (by index).

5. **Build message list:**
   - **If compaction exists:**
     1. Emit `Message::compaction_summary(summary, tokens_before)` as the first message.
     2. Find the position of `first_kept_entry_id` in the path.
     3. Iterate from that position (or compaction index + 1 if not found) to end of path.
     4. For each entry, call `reify_entry()` and collect non-`None` results.
   - **If no compaction:**
     1. Iterate all entries in the path.
     2. For each entry, call `reify_entry()` and collect non-`None` results.

6. **Return** `SessionContext { messages, thinking_level, model }`.

### reify_entry()

The private `reify_entry()` function converts entries to `Message` values:

| Entry Type | Conversion |
|---|---|
| `Message(e)` | `e.message.clone()` |
| `CustomMessage(e)` | `Message::custom(e.custom_type, e.content, e.display, e.details)` |
| `BranchSummary(e)` | `Message::branch_summary(e.summary, e.from_id)` |
| All others | `None` (not reified) |

### Helper functions

```rust
fn entry_id(entry: &FileEntry) -> Option<&EntryId>      // None for Session, Unknown
fn entry_parent_id(entry: &FileEntry) -> Option<&EntryId> // None for Session, Unknown
```

## 8. I/O Patterns

### Lazy Flush

`SessionStore` uses a **lazy flush** pattern controlled by the `ready` flag:

1. **`ready` starts `false`** on `create()` and `in_memory()`. Starts `true` on `open()`.
2. **`flush()` is a no-op** when `ready == false` or `file` is `None`.
3. **`mark_ready()`** sets `ready = true` and immediately attempts a flush (ignoring errors).
4. This lets the caller append entries during initialization without hitting disk, then commit everything once the session is fully set up.

### flush() Behavior

```rust
pub fn flush(&mut self) -> io::Result<()>
```

1. If `file` is `None` or `ready` is `false`, return `Ok(())`.
2. If the file does not yet exist on disk, write the header as the first line (`FileEntry::Session(header)`).
3. Drain the `dirty` buffer, serializing each entry as a single JSON line appended to the file.
4. Uses `BufWriter` for batched I/O.

### mark_ready()

```rust
pub fn mark_ready(&mut self)
```

Sets `ready = true` and calls `flush()`, ignoring any I/O error. This is the trigger that transitions a session from "accumulating" to "persisting."

## 9. Compatibility

- **Version field:** `SessionHeader.version` is always `3`, matching the pi-mono v3 session format.
- **Forward compatibility:** The `#[serde(other)]` attribute on `FileEntry::Unknown` means files written by newer code with additional entry types can be read by older code without errors. Unknown entries are skipped during `open()` and ignored by `build_context()`.
- **Append-only:** The JSONL format is append-only. Entries are never modified or deleted in place. Compaction is achieved by writing a `CompactionEntry` that logically supersedes earlier entries, not by rewriting the file.
- **Line-level atomicity:** Each entry is a single line, so partial writes (e.g., crash mid-flush) corrupt at most one entry. The `open()` parser skips unparseable lines.
