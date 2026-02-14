# TUI Design

How mage renders the agent session in the terminal.


## Architecture

Append-only log in the primary terminal buffer (no alternate screen).
Scrollback preserved. The differential renderer (`Renderer`) diffs each
frame against the previous and repaints only changed rows.

The TUI receives `AgentEvent`s from the agent loop. It does not own the
agent loop.


## Widgets Used (from mage-tui)

| Widget     | Use                                                        |
|------------|------------------------------------------------------------|
| `Renderer` | Frame accumulation, diff rendering, cursor management      |
| `Text`     | Styled multi-span text with padding, bg fill, word-wrap    |
| `Markdown` | Streaming incremental markdown with block-level caching    |
| `HStack`   | Horizontal pane layout for tool blocks (bar | content)     |
| `Editor`   | Multi-line input with cursor positioning                   |

No raw ANSI in the app layer. All styling through `Style`, `Color`,
`Padding`, widget APIs.


## Blocks

The log is a sequence of `Block` values rendered top-to-bottom.
Each block is a **stable element** — it is created once and mutated
in place. The diff renderer detects changes via `Rc::ptr_eq` on
cached lines, so stable blocks that haven't changed cost nothing.


### User Input

```rust
Block::UserInput { text: Text }
```

`Text` widget with bold `>` prefix span. Created on input submit,
never mutated after.


### Command

```rust
Block::Command { input: Text, output: Text }
```

`/` prefixed input. `input` is dim. `output` is whatever the command
handler produced. Both are stable after creation.

Commands intercepted by internal handlers (`/quit`, `/clear`, `/model`,
`/help`) or extension-registered command handlers.


### Assistant Message

```rust
Block::AssistantMessage { markdown: Markdown, streaming: bool }
```

`Markdown` widget with left + right `Padding` and optional bg.
Streams via `markdown.append(delta)` on each `MessageUpdate`.
Block-level caching: only last incomplete block re-renders.
`streaming` flips to false on `MessageEnd`.


### Tool Invocation

```rust
Block::ToolInvocation {
    tool_call_id: String,
    tool_index: usize,
    params: serde_json::Value,
    layout: HStack,           // bar pane (Fixed 1) | content pane (Flex)
    wave: WaveBar,            // animation state
    state: ToolBlockState,
    live: Option<Box<dyn LiveOutput>>,  // type-erased, polls mailbox + renders
}

enum ToolBlockState {
    /// Tool executing. If `live` is Some, the TUI polls and renders it.
    /// If `live` is None, the TUI shows default progress (name + elapsed).
    Running,
    /// Tool finished. Outcome stored for completion rendering.
    Completed {
        result: ToolResult,
        duration: Duration,
    },
}
```

Uses `HStack` with two panes:
- `Fixed(1)` — bar pane. One `║` or `│` character per row, color-animated
  while running, static dim when completed.
- `Flex` — content pane. The tool's render methods own this entire area.

`live` is `Some` only for Custom executions (tier 3). It holds the
type-erased `LiveOutput` which polls the mailbox and delegates to the
tool's `render_progress` / `render_complete`. For sync (tier 1) and
async (tier 2) tools, `live` is `None` and the TUI uses default
rendering functions directly.


## Tool Trait (Reference)

See `DESIGN-TOOL-RENDERING.md` for the full tool execution specification
including `ToolExecution`, `ToolResult`, builders, and the `Mailbox`.

The `Tool` trait defines metadata, execution, LLM output, and rendering
in a single interface. The rendering methods are what the TUI calls
(via `LiveOutput`) each frame:

```rust
pub trait Tool: 'static {
    /// Accumulated state during execution. Default is String.
    type State: 'static = String;

    // ── Metadata ────────────────────────────────────────────────
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> &serde_json::Value;

    // ── Execution ───────────────────────────────────────────────
    fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        cancel: CancelToken,
    ) -> ToolExecution<Self::State>;

    // ── LLM output ──────────────────────────────────────────────
    fn to_result(
        &self,
        result: &ToolResult,
        state: Option<&Self::State>,
    ) -> ToolContent {
        result.result().clone()
    }

    // ── Rendering ───────────────────────────────────────────────

    /// Render in-progress. Only called for Custom (tier 3).
    fn render_progress(
        &self,
        state: Option<&Self::State>,
        params: &serde_json::Value,
        elapsed: Duration,
        r: &mut Renderer,
        width: u16,
    ) {
        default_render_progress(self.name(), params, elapsed, r, width);
    }

    /// Render on completion.
    fn render_complete(
        &self,
        state: Option<&Self::State>,
        result: &ToolResult,
        params: &serde_json::Value,
        elapsed: Duration,
        r: &mut Renderer,
        width: u16,
    ) {
        default_render_complete(self.name(), result, params, elapsed, r, width);
    }
}
```


## Type Erasure (TUI-side)

The `Tool` trait has an associated type, so it's not object-safe. The
registry uses `ErasedTool` + `ToolWrapper<T>` to erase the type (see
`DESIGN-TOOL-RENDERING.md`). What matters to the TUI is what it
receives and calls: `ErasedExecution` and `LiveOutput`.

```rust
pub enum ErasedExecution {
    Ready(ToolResult),
    Running(Pin<Box<dyn Future<Output = ToolResult>>>),
    Custom {
        task: Pin<Box<dyn Future<Output = ToolResult>>>,
        live: Box<dyn LiveOutput>,
    },
}
```

`LiveOutput` is the type-erased interface the TUI calls each frame.
All methods take `&mut self` — the TUI has exclusive ownership.
No RefCell, no boxed closures.

```rust
pub(crate) trait LiveOutput {
    /// Drain the mailbox. Called once per frame before rendering.
    fn poll(&mut self);

    /// Render in-progress state.
    fn render_progress(
        &mut self,
        params: &serde_json::Value,
        elapsed: Duration,
        r: &mut Renderer,
        width: u16,
    );

    /// Render completed state.
    fn render_complete(
        &mut self,
        result: &ToolResult,
        params: &serde_json::Value,
        elapsed: Duration,
        r: &mut Renderer,
        width: u16,
    );
}
```

The concrete bridge: `LiveOutputImpl<T: Tool>` holds `Rc<T>` +
`Mailbox<T::State>` + `Option<T::State>`. It polls the mailbox into
`latest` and delegates render calls to the tool.

```rust
struct LiveOutputImpl<T: Tool> {
    tool: Rc<T>,
    mailbox: Mailbox<T::State>,
    latest: Option<T::State>,
}

impl<T: Tool> LiveOutput for LiveOutputImpl<T> {
    fn poll(&mut self) {
        if let Some(val) = self.mailbox.take() {
            self.latest = Some(val);
        }
    }

    fn render_progress(&mut self, params: &Value, elapsed: Duration, r: &mut Renderer, width: u16) {
        self.tool.render_progress(self.latest.as_ref(), params, elapsed, r, width);
    }

    fn render_complete(&mut self, result: &ToolResult, params: &Value, elapsed: Duration, r: &mut Renderer, width: u16) {
        self.tool.render_complete(self.latest.as_ref(), result, params, elapsed, r, width);
    }
}
```


## Rendering Pipeline

Each frame, the app iterates blocks:

```rust
fn render(&mut self, r: &mut Renderer) {
    for block in &mut self.blocks {
        match block {
            Block::UserInput { text } => {
                text.render(r);
                r.push_blank();
            }
            Block::Command { input, output } => {
                input.render(r);
                output.render(r);
                r.push_blank();
            }
            Block::AssistantMessage { markdown, .. } => {
                markdown.render(r);
                r.push_blank();
            }
            Block::ToolInvocation { state, layout, wave, live, params, .. } => {
                let w = r.width() as usize;
                layout.set_width(w);

                let content_pane = layout.get_mut(CONTENT_PANE);
                let content_w = content_pane.available_width() as u16;
                content_pane.clear();

                match state {
                    ToolBlockState::Running => {
                        let lines = render_to_lines(|r| {
                            match live {
                                Some(live) => {
                                    live.poll();
                                    live.render_progress(params, elapsed, r, content_w);
                                }
                                None => {
                                    default_render_progress(name, params, elapsed, r, content_w);
                                }
                            }
                        }, content_w);
                        for line in &lines {
                            content_pane.push_line(line.clone());
                        }
                    }
                    ToolBlockState::Completed { result, duration } => {
                        let lines = render_to_lines(|r| {
                            match live {
                                Some(live) => {
                                    live.render_complete(result, params, *duration, r, content_w);
                                }
                                None => {
                                    default_render_complete(name, result, params, *duration, r, content_w);
                                }
                            }
                        }, content_w);
                        for line in &lines {
                            content_pane.push_line(line.clone());
                        }
                    }
                }

                // Bar pane
                let bar_pane = layout.get_mut(BAR_PANE);
                bar_pane.clear();
                let height = content_pane.line_count();
                let done = matches!(state, ToolBlockState::Completed { .. });
                for row in 0..height {
                    if done {
                        bar_pane.push_line(/* dim "│" */);
                    } else {
                        bar_pane.push_line(wave.styled_char(row));
                    }
                }

                r.push_lines(layout.compose());
                r.push_blank();
            }
        }
    }

    if self.awaiting_input {
        self.editor.render(r);
    }
}
```


### render_to_lines Helper

**Resolved**: tools render into `Renderer` (full widget access). A capture
helper extracts the accumulated lines.

```rust
fn render_to_lines(f: impl FnOnce(&mut Renderer), width: u16) -> Vec<Line> {
    let mut r = Renderer::new();
    r.begin_frame(width, u16::MAX);
    f(&mut r);
    std::mem::take(&mut r.lines)  // needs pub(crate) access or a drain method
}
```

Tools get full `Renderer` access — they can use `Text`, `Markdown`, raw
`push_line`, whatever they need. The capture helper creates a throwaway
`Renderer`, collects lines, and feeds them into the `Pane`.


## Complete Examples

See `DESIGN-TOOL-RENDERING.md` for tier 1 (sync) and tier 2 (async) examples.
Those tiers use default rendering and have no TUI-specific code.

### Tier 3: Custom Tool (bash)

This example shows custom `render_progress` / `render_complete` — the
TUI-relevant rendering code:

```rust
struct BashTool;

struct BashState {
    command: String,
    stdout_lines: Vec<String>,
    exit_code: Option<i32>,
}

impl Tool for BashTool {
    type State = BashState;

    fn name(&self) -> &str { "bash" }
    fn description(&self) -> &str { "Run a shell command" }
    fn parameters(&self) -> &serde_json::Value { &BASH_SCHEMA }

    fn execute(&self, _id: &str, params: Value, cancel: CancelToken) -> ToolExecution<BashState> {
        let command = params["command"].as_str().unwrap_or("").to_string();

        ToolExecution::custom(|tx| Box::pin(async move {
            tx.send(BashState {
                command: command.clone(),
                stdout_lines: vec![],
                exit_code: None,
            });

            let mut child = Command::new("sh")
                .arg("-c").arg(&command)
                .stdout(Stdio::piped())
                .spawn()?;

            let mut lines = Vec::new();
            while let Some(line) = read_line(&mut child).await {
                lines.push(line.clone());
                tx.send(BashState {
                    command: command.clone(),
                    stdout_lines: lines.clone(),
                    exit_code: None,
                });
            }

            let code = child.wait().await?.code().unwrap_or(-1);
            if code == 0 {
                ToolResult::success(lines.join("\n"))
            } else {
                ToolResult::failure(format!("exit code {code}\n{}", lines.join("\n")))
            }
        }))
    }

    fn render_progress(
        &self,
        state: Option<&BashState>,
        _params: &Value,
        elapsed: Duration,
        r: &mut Renderer,
        width: u16,
    ) {
        if let Some(state) = state {
            let mut title = Text::empty();
            title.push("bash", Style::new().bold());
            title.push(&format!("  {}", state.command), Style::new().dim());
            title.push(&format!("  {:.1}s", elapsed.as_secs_f32()), Style::new().dim());
            title.render(r);

            let show = state.stdout_lines.len().min(10);
            for line in &state.stdout_lines[state.stdout_lines.len() - show..] {
                r.push_line(line.as_str());
            }
            if state.stdout_lines.len() > 10 {
                Text::new(format!("… ({} more)", state.stdout_lines.len() - 10))
                    .style(Style::new().dim())
                    .render(r);
            }
        } else {
            default_render_progress(self.name(), _params, elapsed, r, width);
        }
    }

    fn render_complete(
        &self,
        state: Option<&BashState>,
        result: &ToolResult,
        _params: &Value,
        elapsed: Duration,
        r: &mut Renderer,
        width: u16,
    ) {
        let command = state.map(|s| s.command.as_str()).unwrap_or("?");

        let mut title = Text::empty();
        title.push("bash", Style::new().bold());
        title.push(&format!("  {command}"), Style::new().dim());
        title.push(&format!("  {:.1}s", elapsed.as_secs_f32()), Style::new().dim());
        if result.is_error() {
            title.set_bg(Some(Color::Rgb(50, 20, 20)));
        } else {
            title.set_bg(Some(Color::Rgb(20, 40, 20)));
        }
        title.render(r);

        if let Some(state) = state {
            let show = state.stdout_lines.len().min(20);
            for line in &state.stdout_lines[state.stdout_lines.len() - show..] {
                r.push_line(line.as_str());
            }
            if state.stdout_lines.len() > 20 {
                Text::new(format!("… ({} more)", state.stdout_lines.len() - 20))
                    .style(Style::new().dim())
                    .render(r);
            }
        }
    }

    fn to_result(&self, result: &ToolResult, _state: Option<&BashState>) -> ToolContent {
        result.result().clone()
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
