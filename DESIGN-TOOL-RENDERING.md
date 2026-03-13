# Tool Rendering Design

## Overview

Tools are closures registered via `ExtensionRegistry`. During execution, each
tool receives a `ToolHandle` for cancellation checks and streaming progress.
The agent loop emits `AgentEvent` variants that the TUI consumes for rendering.
No trait-based rendering — the TUI owns the presentation.


## Tool Registration

Tools are registered during extension init via `ExtensionRegistry::tool()`:

```rust
pub fn tool<F, Fut>(&mut self, schema: llm::Tool, execute: F)
where
    F: Fn(String, serde_json::Value, ToolHandle) -> Fut + 'static,
    Fut: Future<Output = ToolResult> + 'static,
```

Internally, the closure is boxed into a `RegisteredTool`:

```rust
pub struct RegisteredTool {
    pub schema: llm::Tool,
    pub execute: Box<
        dyn Fn(String, serde_json::Value, ToolHandle) -> Pin<Box<dyn Future<Output = ToolResult>>>
    >,
}
```

No trait to implement. No struct to define. A closure is sufficient.


## ToolHandle

Each tool invocation receives a `ToolHandle` — a per-call handle created by
the agent loop.

```rust
pub struct ToolHandle {
    pub tool_call_id: String,
    cancel: CancelToken,
    update_tx: tokio::sync::mpsc::UnboundedSender<ToolUpdate>,
    loop_handle: LoopHandle,
}
```

Methods:

| Method | Purpose |
|---|---|
| `is_cancelled() -> bool` | Sync check — call at yield points in long-running tools |
| `cancel_token() -> &CancelToken` | Pass the token deeper into subtasks |
| `send_update(ToolUpdate)` | Fire-and-forget progress push to the UI (unbounded channel) |
| `loop_handle() -> &LoopHandle` | Access the loop handle for message injection, steering, etc. |

`send_update` pushes a `ToolUpdate` through an unbounded mpsc channel. The
agent loop drains these and emits `AgentEvent::ToolExecUpdate` for each.


## ToolUpdate

Progress sent from a running tool to the UI:

```rust
pub struct ToolUpdate {
    pub content: Vec<llm::UserContent>,
    pub metadata: Option<serde_json::Value>,
}
```

- `content` — text, images, or any `UserContent` the TUI can render.
- `metadata` — optional structured data for tool-specific UI (e.g. line count, exit code).


## ToolResult

The final outcome of a tool execution:

```rust
pub struct ToolResult {
    pub content: Vec<llm::UserContent>,
    pub is_error: bool,
}
```

Constructors:

| Constructor | Produces |
|---|---|
| `ToolResult::success(s)` | `{ content: [Text(s)], is_error: false }` |
| `ToolResult::failure(s)` | `{ content: [Text(s)], is_error: true }` |
| `ToolResult::skipped()` | `failure("Skipped (interrupted)")` |

The content is sent back to the LLM as the tool result. No intermediate
`ToolContent` type — just `Vec<llm::UserContent>` directly.


## Agent Event Lifecycle

The agent loop emits three events per tool call:

```
ToolExecStart { tool_call_id, tool_name, args }
    │
    ├── ToolExecUpdate { tool_call_id, tool_name, update }   (0..n)
    │
ToolExecEnd { tool_call_id, tool_name, result }
```

```rust
pub enum AgentEvent {
    // ...
    ToolExecStart {
        tool_call_id: Str,
        tool_name: Str,
        args: serde_json::Value,
    },
    ToolExecUpdate {
        tool_call_id: Str,
        tool_name: Str,
        update: ToolUpdate,
    },
    ToolExecEnd {
        tool_call_id: Str,
        tool_name: Str,
        result: ToolResult,
    },
    // ...
}
```


## TUI Rendering

The TUI subscribes to `AgentEvent` and renders tool widgets:

1. **`ToolExecStart`** — create a tool widget, show tool name + args preview + spinner.
2. **`ToolExecUpdate`** — update the widget with new content from `ToolUpdate`.
   The TUI decides presentation: streaming text, progress bars, structured
   metadata display. This is entirely TUI-owned logic.
3. **`ToolExecEnd`** — transition the widget to completed state. Show result
   content, tint background green (success) or red (failure).

No rendering traits or callbacks flow from core to TUI. The core emits events;
the TUI interprets them. Tool authors control _what_ is sent (via
`send_update` and the return value); the TUI controls _how_ it looks.


## Complete Example

```rust
reg.tool(
    llm::Tool {
        name: "bash".into(),
        description: "Run a shell command".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command to run" }
            },
            "required": ["command"]
        }),
    },
    |_id, params, handle| async move {
        let cmd = params["command"].as_str().unwrap_or("").to_string();

        // Stream initial state
        handle.send_update(ToolUpdate {
            content: vec![llm::UserContent::Text { text: format!("$ {cmd}") }],
            metadata: None,
        });

        let mut child = Command::new("sh")
            .arg("-c").arg(&cmd)
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{e}"))?;

        let mut lines = Vec::new();
        while let Some(line) = read_line(&mut child).await {
            lines.push(line.clone());

            // Check cancellation at each yield point
            if handle.is_cancelled() {
                let _ = child.kill();
                return ToolResult::skipped();
            }

            // Stream progress — TUI sees ToolExecUpdate
            handle.send_update(ToolUpdate {
                content: vec![llm::UserContent::Text {
                    text: lines.join("\n"),
                }],
                metadata: Some(serde_json::json!({ "line_count": lines.len() })),
            });
        }

        let code = child.wait().await.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1);

        if code == 0 {
            ToolResult::success(lines.join("\n"))
        } else {
            ToolResult::failure(format!("exit code {code}\n{}", lines.join("\n")))
        }
    },
);
```


## Summary

- **Registration**: closures via `ExtensionRegistry::tool(schema, closure)`.
- **Handle**: `ToolHandle` per call — cancellation, progress, loop access.
- **Progress**: `ToolUpdate` pushed via `send_update`, emitted as `ToolExecUpdate`.
- **Result**: `ToolResult { content, is_error }` — flat struct, not enum.
- **Events**: `ToolExecStart` → `ToolExecUpdate`* → `ToolExecEnd`.
- **Rendering**: TUI-owned. Core emits events; TUI decides presentation.
