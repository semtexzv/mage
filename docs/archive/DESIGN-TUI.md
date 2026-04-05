# TUI Design

How mage renders the agent session in the terminal.


## Architecture
Append-only log in the primary terminal buffer (no alternate screen).
Scrollback preserved. The differential renderer (`Renderer`) diffs each
frame against the previous and repaints only changed rows.
The TUI receives `AgentEvent`s from the agent loop. It does not own the
agent loop.


## Core Principle: Widget-Per-Entry

Every element in the conversation log owns a **persistent widget**
(`Text`, `Markdown`, or a composite struct containing them). Widgets
cache their rendered `Rc<str>` lines internally. The render pass is a
trivial loop:

```rust
for entry in &mut self.log {
    r.push_blank();
    match entry {
        LogEntry::User(text)      => text.render(r),
        LogEntry::Assistant(md)   => md.render(r),
        LogEntry::Tool(tw)        => tw.render(r),
        LogEntry::Error(text)     => text.render(r),
    }
}
```

**No raw ANSI, no `push_line` with format strings, no manual bg fills
in the app layer.** All styling flows through `Style`, `Color`,
`Padding`, and widget APIs. The app creates widgets via factory
functions (`make_user_text`, `make_assistant_md`, etc.) that set the
theme once; widgets handle their own padding, bg fill, word-wrap,
and caching.

This matters for three reasons:

1. **Diff efficiency.** Unchanged widgets produce the same `Rc<str>`
   line pointers. The renderer skips repainting them via `Rc::ptr_eq`.
2. **Modularity.** Widget creation is separated from rendering. The
   render loop doesn't know how a `Text` fills its background — it
   just calls `.render(r)`.
3. **Testability.** Widgets can be rendered into any `LineSink`
   (trait), not just the real `Renderer`. Tests can inspect cached
   lines without a terminal.


## Widgets (from mage-tui)
| Widget       | Use                                                         |
|--------------|-------------------------------------------------------------|
| `Renderer`   | Frame accumulation, diff rendering, cursor management       |
| `LineSink`   | Trait — width + push_lines; `Renderer` implements it        |
| `View`       | Trait — `render(&mut self, &mut impl LineSink)`             |
| `Text`       | Styled multi-span text with padding, bg fill, word-wrap     |
| `Markdown`   | Streaming incremental markdown with block-level caching     |
| `Editor`     | Multi-line input with cursor positioning                    |
| `HStack`     | Horizontal pane layout for tool blocks (future)             |

## Log Entries

The log is `Vec<LogEntry>`. Each variant owns its widget(s):

```rust
enum LogEntry {
    User(Text),            // Immutable after creation
    Assistant(Markdown),   // Streamed via .append(delta)
    Tool(ToolWidget),      // Composite: header Text + optional output Text
    Error(Text),           // Immutable after creation
}
```

### User

`Text` with bold style, `BG_USER` background, `Padding::new(1,1,1,1)`.
Created on submit, never mutated.

### Assistant

`Markdown` with `Padding::new(1,1,1,1)` and `BG_ASSISTANT` background.
Streams via `markdown.append(delta)` on each `MessageDelta`.
Block-level caching: only the last incomplete block re-renders.

### Tool

`ToolWidget` — a composite widget holding:
- `header: Text` — icon + tool name + args summary, with `BG_TOOL` bg
- `output: Option<Text>` — populated on completion with result text

On `ToolExecStart`, a running `ToolWidget` is created with a `⏵`
icon. Streaming progress arrives via `ToolExecUpdate` events. On
`ToolExecEnd`, `complete()` rebuilds the header with `✓`/`✗` icon
and populates the output text.

```rust
struct ToolWidget {
    name: String,
    header: Text,
    output: Option<Text>,
}
```

### Error

`Text` with bold red "error:" prefix, `BG_ERROR` background.
Immutable after creation.

### Tool Invocation (future: HStack + streaming updates)

The current implementation uses a simple composite `ToolWidget` with
a header `Text` and optional output `Text`. The aspirational design
uses `HStack` with a wave-animated bar pane:
```rust
Block::ToolInvocation {
    tool_call_id: String,
    tool_index: usize,
    params: serde_json::Value,
    layout: HStack,           // bar pane (Fixed 1) | content pane (Flex)
    wave: WaveBar,            // animation state
    state: ToolBlockState,
}
```

Streaming tool progress is delivered via `AgentEvent::ToolExecUpdate`
events, which carry a `ToolUpdate { content, metadata }`. The TUI can
use these to update the tool widget's output in real time.

This is not yet implemented. The current `ToolWidget` is the v1
simplification that satisfies the widget-per-entry principle.


## Closure-Based Tools (Reference)

See `DESIGN-TOOL-RENDERING.md` for the full tool execution specification.

Tools are registered as closures via `ExtensionRegistry::tool(schema, closure)`.
Each tool receives its arguments and a `ToolHandle` for cancellation and
streaming updates:

```rust
pub struct RegisteredTool {
    pub schema: llm::Tool,
    pub execute: Box<dyn Fn(String, Value, ToolHandle) -> Pin<Box<dyn Future<Output = ToolResult>>>>,
}
```

`ToolHandle` provides per-tool context during execution:

```rust
pub struct ToolHandle {
    // Check if the tool invocation has been cancelled
    pub fn is_cancelled(&self) -> bool;
    pub fn cancel_token(&self) -> CancelToken;

    // Stream progress updates to the TUI
    pub fn send_update(&self, update: ToolUpdate);

    // Access the loop handle for injecting messages, etc.
    pub fn loop_handle(&self) -> &LoopHandle;
}
```

The `ToolResult` struct replaces the old enum:

```rust
pub struct ToolResult {
    pub content: Vec<llm::UserContent>,
    pub is_error: bool,
}
```


## Tool Event Flow (TUI-side)

The TUI no longer uses type-erased `LiveOutput` or `Mailbox`. Instead,
it reacts to `AgentEvent` variants emitted by the agent loop:

1. **`ToolExecStart { tool_call_id, tool_name, args }`** — create a
   running `ToolWidget` with `⏵` icon.
2. **`ToolExecUpdate { tool_call_id, tool_name, update }`** — update
   the tool widget's output with streaming progress. The `ToolUpdate`
   carries `content: Vec<UserContent>` and optional `metadata`.
3. **`ToolExecEnd { tool_call_id, tool_name, result }`** — finalize
   the widget: rebuild header with `✓`/`✗` icon based on
   `result.is_error`, populate output from `result.content`.

This is simpler than the old `ErasedExecution`/`LiveOutput` design:
the TUI is a pure event consumer with no polling or type erasure.


## Rendering Pipeline
Each frame, the app iterates log entries. Widgets render themselves;
the app never constructs ANSI strings or calls `push_line` directly.

```rust
fn render(&mut self, r: &mut Renderer) {
    // Chat log — each entry renders itself.
    for entry in &mut self.log {
        r.push_blank();
        match entry {
            LogEntry::User(text)      => text.render(r),
            LogEntry::Assistant(md)   => md.render(r),
            LogEntry::Tool(tw)        => tw.render(r),
            LogEntry::Error(text)     => text.render(r),
        }
    }

    // Thinking indicator (persistent widget, only shown while running).
    if self.running {
        r.push_blank();
        self.thinking.render(r);
    }

    // Input area with border rules.
    r.push_blank();
    push_hr(r, FG_BORDER);
    self.editor.render(r, " ");
    push_hr(r, FG_BORDER);

    // Status bar (persistent widget, updated on state change).
    self.status.render(r);
}
```

Chrome elements (thinking indicator, status bar) are also persistent
`Text` widgets stored on the TUI struct. They are recreated via factory
functions only when state changes (e.g. `set_running(true)` rebuilds
both).

### Factory Functions

Widget creation is separated from rendering via factory functions:

```rust
fn make_user_text(content: &str) -> Text { ... }    // bold, BG_USER, PAD
fn make_assistant_md(width: u16) -> Markdown { ... } // BG_ASSISTANT, PAD
fn make_error_text(message: &str) -> Text { ... }    // bold red prefix, BG_ERROR, PAD
fn make_status_text(label: &str) -> Text { ... }     // dim, BG_STATUS, PAD_H
fn make_thinking_text() -> Text { ... }               // dim, BG_STATUS, PAD
```

Theme colors are constants at the top of `tui.rs`. Changing a color
changes every widget that uses it.

### LineSink Trait

Widgets render into `impl LineSink` (not `Renderer` directly). This
allows testing without a terminal and enables nested containers.

```rust
pub trait LineSink {
    fn width(&self) -> u16;
    fn push_lines(&mut self, lines: &[Line]);
}

impl LineSink for Renderer { ... }
```

`View` is the matching trait for widgets:

```rust
pub trait View {
    fn render(&mut self, sink: &mut impl LineSink);
}
```

`Text` implements `View`. `Markdown` currently takes `&mut Renderer`
directly (it predates `LineSink`); migrating it is a future cleanup.

### render_to_lines Helper (future)

For tool custom rendering, a capture helper will extract lines from a
throwaway renderer:

```rust
fn render_to_lines(f: impl FnOnce(&mut Renderer), width: u16) -> Vec<Line> {
    let mut r = Renderer::new();
    r.begin_frame(width, u16::MAX);
    f(&mut r);
    std::mem::take(&mut r.lines)
}
```


## Complete Examples

See `DESIGN-TOOL-RENDERING.md` for tool rendering details.
Tools are closures; rendering tiers are handled by the TUI based on
`AgentEvent` variants.

### Example: Bash Tool (closure-based)

This example shows how a bash tool uses `ToolHandle::send_update()` for
streaming output, and how the TUI renders the resulting events:

```rust
// Registration (in an Extension's init method):
registry.tool(bash_schema(), |tool_call_id, params, handle| {
    Box::pin(async move {
        let command = params["command"].as_str().unwrap_or("").to_string();

        let mut child = Command::new("sh")
            .arg("-c").arg(&command)
            .stdout(Stdio::piped())
            .spawn()?;

        let mut lines = Vec::new();
        while let Some(line) = read_line(&mut child).await {
            lines.push(line.clone());
            handle.send_update(ToolUpdate {
                content: vec![UserContent::Text(lines.join("\n"))],
                metadata: None,
            });
        }

        let code = child.wait().await?.code().unwrap_or(-1);
        let output = lines.join("\n");
        ToolResult {
            content: vec![UserContent::Text(if code == 0 {
                output
            } else {
                format!("exit code {code}\n{output}")
            })],
            is_error: code != 0,
        }
    })
});
```

The TUI handles rendering based on the event stream:

```rust
// TUI event handling:
AgentEvent::ToolExecStart { tool_call_id, tool_name, args } => {
    // Create running ToolWidget with ⏵ icon
    let tw = ToolWidget::new_running(&tool_name, &args);
    self.log.push(LogEntry::Tool(tw));
}
AgentEvent::ToolExecUpdate { tool_call_id, update, .. } => {
    // Update widget output with streaming content
    if let Some(tw) = self.find_tool_widget(&tool_call_id) {
        tw.update_output(&update.content);
    }
}
AgentEvent::ToolExecEnd { tool_call_id, result, .. } => {
    // Finalize: ✓/✗ icon, populate output from result.content
    if let Some(tw) = self.find_tool_widget(&tool_call_id) {
        tw.complete(&result);
    }
}
```


## Tick Management

~80ms interval, only while a tool is running.

```rust
Event::Message(Tick) => {
    for block in &mut self.blocks {
        if let Block::ToolInvocation { state, wave, .. } = block {
            if matches!(state, ToolBlockState::Running) {
                wave.tick();
            }
        }
    }
}
```

Timer starts on `ToolExecStart`, stops when last running tool completes.


## Wave Bar

One character wide. `║` while running, `│` when completed.

```rust
struct WaveBar {
    phase: f32,
    freq: f32,     // peaks per height
    speed: f32,    // phase increment per tick
    active: Color, // peak color
    dim: Color,    // trough color
}

impl WaveBar {
    fn tick(&mut self) {
        self.phase += self.speed;
    }

    fn styled_char(&self, row: usize) -> Line {
        let t = ((row as f32 * self.freq - self.phase).sin() + 1.0) / 2.0;
        let color = lerp_color(self.dim, self.active, t);
        Rc::from(format!("{}\u{2551}{}", Style::new().fg(color).to_sgr(), RESET).as_str())
    }

    fn static_char(dim: Color) -> Line {
        Rc::from(format!("{}\u{2502}{}", Style::new().fg(dim).to_sgr(), RESET).as_str())
    }
}
```


## Theme

```rust
// Added to Theme:
pub tool_success_bg: Color,  // subtle green tint for title bg
pub tool_error_bg: Color,    // subtle red tint for title bg
pub tool_bar_active: Color,  // wave peak
pub tool_bar_dim: Color,     // wave trough / completed bar
pub tool_name: Style,        // bold
pub tool_args: Style,        // dim
pub tool_elapsed: Style,     // dim
pub tool_content: Style,     // most de-emphasized
```


## Open Questions

- **Scroll control**: viewport scrolling during streaming. Not v1.
- **Image content blocks**: placeholder for v1.
- **Thinking blocks**: collapsible dim/italic? Suppress?
- **Tool output collapse/expand**: keyboard-driven, deferred.
- **Command registry**: how extensions register commands.
- **Memory**: long-running tools with large output.
