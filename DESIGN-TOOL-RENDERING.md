# Tool Rendering Design

## The Tool Trait

One trait. Everything about a tool — metadata, execution, rendering — lives here.

```rust
pub trait Tool: 'static {
    /// Accumulated state during execution. Sent through a mailbox.
    /// Default is `String` — the implicit progress/output text shown
    /// by the default renderer.
    type State: 'static = String;

    // ── Metadata (for the LLM) ──────────────────────────────────

    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> &serde_json::Value;

    // ── Execution ───────────────────────────────────────────────

    /// Execute the tool. Returns a ToolExecution.
    fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        cancel: CancelToken,
    ) -> ToolExecution<Self::State>;

    // ── LLM output ──────────────────────────────────────────────

    /// Convert the final state into the content sent back to the LLM.
    /// Default: uses the ToolResult's text directly.
    /// Override for tools where the state contains richer data than
    /// the result text (e.g. structured JSON, truncated output).
    fn to_result(
        &self,
        result: &ToolResult,
        state: Option<&Self::State>,
    ) -> ToolContent {
        result.result().clone()
    }

    // ── Rendering (optional, has defaults) ──────────────────────

    /// Render while in progress. Only called for Custom executions.
    ///
    /// `state`: latest state from the mailbox, or None if nothing sent yet.
    /// Renders the ENTIRE content pane (title, args, progress, output).
    ///
    /// Default: shows tool name + state text (if String) + elapsed.
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
    ///
    /// `state`: final state from the mailbox, or None for simple/async tools.
    /// `result`: success or failure.
    /// Renders the ENTIRE content pane (title, result, output).
    ///
    /// Default: shows tool name + result text preview + bg tint.
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


## Three Execution Tiers

Tools return a `ToolExecution` from `execute()`. Three tiers, from
simplest to most powerful:


### Tier 1: Sync (`ToolResult` → `ToolExecution`)

For tools that do trivial synchronous work. Just return a result.

```rust
impl From<ToolResult> for ToolExecution<String> {
    fn from(result: ToolResult) -> Self {
        ToolExecution::Ready(result)
    }
}
```

Usage — the simplest possible tool:

```rust
fn execute(&self, _id: &str, params: Value, _cancel: CancelToken) -> ToolExecution {
    match std::fs::write(&path, &content) {
        Ok(()) => ToolResult::success(format!("{} bytes written")).into(),
        Err(e) => ToolResult::failure(format!("{e}")).into(),
    }
}
```

No async, no future, no mailbox. The agent loop sees `Ready` and
immediately consumes the result.


### Tier 2: Async (future, no custom state)

For tools that do async I/O but don't need streaming output.
Uses the default `String` state — the TUI shows tool name + elapsed.

```rust
fn execute(&self, _id: &str, params: Value, _cancel: CancelToken) -> ToolExecution {
    ToolExecution::running(async move {
        let body = reqwest::get(&url).await?.text().await?;
        ToolResult::success(body)
    })
}
```

The TUI shows default progress (tool name, params preview, elapsed).
When the future completes, default completion rendering.


### Tier 3: Custom (future + mailbox + custom rendering)

For tools with streaming output and custom rendering.
The tool defines a custom `State` type and overrides `render_progress`
and/or `render_complete`.

```rust
type State = BashState;

fn execute(&self, _id: &str, params: Value, cancel: CancelToken) -> ToolExecution<BashState> {
    ToolExecution::custom(|tx| Box::pin(async move {
        tx.send(BashState { command: cmd.clone(), lines: vec![], .. });
        // ... stream output, send updates ...
        ToolResult::success(output)
    }))
}
```


## ToolExecution
```rust
pub enum ToolExecution<S: 'static = String> {
    /// Tier 1 — Sync. Result ready immediately.
    Ready(ToolResult),
    /// Tier 2 — Async. Future, default rendering.
    Running(Pin<Box<dyn Future<Output = ToolResult>>>),
    /// Tier 3 — Custom. Future + mailbox + custom rendering.
    Custom {
        task: Pin<Box<dyn Future<Output = ToolResult>>>,
        mailbox: Mailbox<S>,
    },
}
```

Builder methods (accept closures, box internally):

```rust
impl ToolExecution<String> {
    /// Tier 2: async, default rendering.
    pub fn running(
        task: impl Future<Output = ToolResult> + 'static,
    ) -> Self {
        ToolExecution::Running(Box::pin(task))
    }
}
impl<S: 'static> ToolExecution<S> {
    /// Tier 3: custom state + mailbox.
    /// Accepts a closure that receives a MailboxSender and returns a future.
    /// Boxing is done internally — the caller writes a plain async block.
    pub fn custom(
        task: impl FnOnce(MailboxSender<S>) -> Pin<Box<dyn Future<Output = ToolResult>>>,
    ) -> Self {
        let (tx, mailbox) = Mailbox::new();
        ToolExecution::Custom {
            task: task(tx),
            mailbox,
        }
    }
}
/// Tier 1: sync result.
impl<S: 'static> From<ToolResult> for ToolExecution<S> {
    fn from(result: ToolResult) -> Self {
        ToolExecution::Ready(result)
    }
}
```


## ToolResult and ToolContent

```rust
/// The result of a tool execution.
pub enum ToolResult {
    /// Tool succeeded. Content goes back to the LLM.
    Success(ToolContent),
    /// Tool failed. Error message goes back to the LLM.
    Failure(ToolContent),
}

/// The content returned to the LLM.
#[derive(Clone)]
pub struct ToolContent {
    pub content: Vec<UserContent>,
}

impl ToolContent {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            content: vec![UserContent::Text { text: s.into() }],
        }
    }

    pub fn json(value: &impl Serialize) -> Self {
        Self {
            content: vec![UserContent::Text {
                text: serde_json::to_string_pretty(value).unwrap_or_default(),
            }],
        }
    }
}

impl ToolResult {
    pub fn success(s: impl Into<String>) -> Self {
        Self::Success(ToolContent::text(s))
    }

    pub fn failure(s: impl Into<String>) -> Self {
        Self::Failure(ToolContent::text(s))
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Self::Failure(_))
    }

    pub fn result(&self) -> &ToolContent {
        match self {
            Self::Success(r) | Self::Failure(r) => r,
        }
    }
}
```


## Mailbox

Single-slot latest-value container. Writer overwrites. Reader takes latest.
No queue, no backpressure, zero allocation per send (the slot is pre-allocated).

```rust
pub struct Mailbox<T> {
    inner: Rc<MailboxInner<T>>,
}

pub struct MailboxSender<T> {
    inner: Rc<MailboxInner<T>>,
}

struct MailboxInner<T> {
    value: Cell<Option<T>>,
}

impl<T> Mailbox<T> {
    pub fn new() -> (MailboxSender<T>, Mailbox<T>) {
        let inner = Rc::new(MailboxInner {
            value: Cell::new(None),
        });
        (
            MailboxSender { inner: inner.clone() },
            Mailbox { inner },
        )
    }

    /// Take the latest value (if any). Returns None if no new
    /// value since last take.
    pub fn take(&self) -> Option<T> {
        self.inner.value.take()
    }
}

impl<T> MailboxSender<T> {
    /// Send a new value, overwriting any previous unseen value.
    pub fn send(&self, value: T) {
        self.inner.value.set(Some(value));
    }
}
```

`Cell<Option<T>>` — no RefCell, no borrow panics. `Cell::set` overwrites,
`Cell::take` extracts. Both are `&self`.


## Type Erasure

The `Tool` trait has an associated type, so it's not object-safe. Two layers
of erasure: `ErasedTool` for the registry, `LiveOutput` for rendering.

```rust
/// Object-safe interface for the tool registry and agent loop.
pub(crate) trait ErasedTool {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> &serde_json::Value;

    fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        cancel: CancelToken,
    ) -> ErasedExecution;

    /// Convert result + state into what gets sent to the LLM.
    fn to_result(
        &self,
        result: &ToolResult,
        live: Option<&dyn LiveOutput>,
    ) -> ToolContent;

    /// Render completed state (for tools without LiveOutput).
    fn render_complete_default(
        &self,
        result: &ToolResult,
        params: &serde_json::Value,
        elapsed: Duration,
        r: &mut Renderer,
        width: u16,
    );
}

/// Type-erased execution returned by ErasedTool::execute.
pub enum ErasedExecution {
    /// Sync result, ready immediately.
    Ready(ToolResult),
    /// Async, no custom state. Default rendering.
    Running(Pin<Box<dyn Future<Output = ToolResult>>>),
    /// Async with custom live output.
    Custom {
        task: Pin<Box<dyn Future<Output = ToolResult>>>,
        live: Box<dyn LiveOutput>,
    },
}
```

### LiveOutput (`&mut self`, no RefCell, no boxed closures)

The TUI owns `Box<dyn LiveOutput>`. All methods take `&mut self`.
`latest` is a plain `Option<T::State>` — no interior mutability.
Render methods delegate to the concrete `Tool` via `Rc<T>`.

```rust
/// Type-erased live output. Owned by the TUI's tool block.
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

The concrete implementation:

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

    fn render_progress(
        &mut self,
        params: &serde_json::Value,
        elapsed: Duration,
        r: &mut Renderer,
        width: u16,
    ) {
        self.tool.render_progress(self.latest.as_ref(), params, elapsed, r, width);
    }

    fn render_complete(
        &mut self,
        result: &ToolResult,
        params: &serde_json::Value,
        elapsed: Duration,
        r: &mut Renderer,
        width: u16,
    ) {
        self.tool.render_complete(self.latest.as_ref(), result, params, elapsed, r, width);
    }
}
```

No `RefCell`. No `Box<dyn Fn>`. No closures. Just `Rc<T>` and `Option<T::State>`.


### ErasedTool wrapper

```rust
struct ToolWrapper<T: Tool> {
    tool: Rc<T>,
}

impl<T: Tool> ErasedTool for ToolWrapper<T> {
    fn name(&self) -> &str { self.tool.name() }
    fn description(&self) -> &str { self.tool.description() }
    fn parameters(&self) -> &serde_json::Value { self.tool.parameters() }

    fn execute(
        &self,
        tool_call_id: &str,
        params: serde_json::Value,
        cancel: CancelToken,
    ) -> ErasedExecution {
        let exec = self.tool.execute(tool_call_id, params, cancel);
        match exec {
            ToolExecution::Ready(result) => ErasedExecution::Ready(result),
            ToolExecution::Running(task) => ErasedExecution::Running(task),
            ToolExecution::Custom { task, mailbox } => ErasedExecution::Custom {
                task,
                live: Box::new(LiveOutputImpl {
                    tool: self.tool.clone(),  // Rc clone, cheap
                    mailbox,
                    latest: None,
                }),
            },
        }
    }

    fn to_result(
        &self,
        result: &ToolResult,
        live: Option<&dyn LiveOutput>,
    ) -> ToolContent {
        // LiveOutput can't give us &T::State through dyn — but we can
        // have LiveOutputImpl store the final state and expose it.
        // For now, delegate to Tool::to_result with None state for
        // non-custom, or final polled state for custom.
        // (Implementation detail — the typed state is inside LiveOutputImpl,
        // accessed via a downcast or an additional trait method.)
        self.tool.to_result(result, None)
    }

    fn render_complete_default(
        &self,
        result: &ToolResult,
        params: &serde_json::Value,
        elapsed: Duration,
        r: &mut Renderer,
        width: u16,
    ) {
        self.tool.render_complete(None, result, params, elapsed, r, width);
    }
}
```


## Agent Loop Integration

The agent loop handles the three tiers:

```rust
let erased_exec = tool.execute(tool_call_id, params, cancel);

let result = match erased_exec {
    // Tier 1: sync — result is ready now
    ErasedExecution::Ready(result) => result,
    // Tier 2: async — await the future, show default progress
    ErasedExecution::Running(task) => {
        // TUI shows default running display (tool name + elapsed)
        task.await
    }
    // Tier 3: custom — await the future, TUI polls live output
    ErasedExecution::Custom { task, live } => {
        // Hand `live` to the TUI block for rendering
        tui.set_tool_live(tool_call_id, live);
        task.await
    }
};

// Get what to send to the LLM
let content = tool.to_result(&result, tui.get_tool_live(tool_call_id));
// Transition TUI block to completed state
tui.tool_completed(tool_call_id, result, elapsed);
```


## Complete Examples

### Tier 1: Sync Tool (write_file)

```rust
struct WriteFileTool;

impl Tool for WriteFileTool {
    // State = String (default), never used

    fn name(&self) -> &str { "write_file" }
    fn description(&self) -> &str { "Write content to a file" }
    fn parameters(&self) -> &serde_json::Value { &WRITE_FILE_SCHEMA }

    fn execute(&self, _id: &str, params: Value, _cancel: CancelToken) -> ToolExecution {
        let path = params["path"].as_str().unwrap_or("").to_string();
        let content = params["content"].as_str().unwrap_or("").to_string();

        match std::fs::write(&path, &content) {
            Ok(()) => ToolResult::success(format!("{} bytes written to {path}", content.len())).into(),
            Err(e) => ToolResult::failure(format!("{e}")).into(),
        }
    }
}
```

Three methods. No async. No state. No rendering overrides.


### Tier 2: Async Tool (fetch_url)

```rust
struct FetchUrlTool;

impl Tool for FetchUrlTool {
    // State = String (default), never used

    fn name(&self) -> &str { "fetch_url" }
    fn description(&self) -> &str { "Fetch a URL" }
    fn parameters(&self) -> &serde_json::Value { &FETCH_URL_SCHEMA }

    fn execute(&self, _id: &str, params: Value, _cancel: CancelToken) -> ToolExecution {
        let url = params["url"].as_str().unwrap_or("").to_string();

        ToolExecution::running(async move {
            match reqwest::get(&url).await.and_then(|r| r.text().await) {
                Ok(body) => ToolResult::success(body),
                Err(e) => ToolResult::failure(format!("{e}")),
            }
        })
    }
}
```

Four methods worth of code. Default rendering throughout.


### Tier 3: Custom Tool (bash)

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

    fn to_result(&self, result: &ToolResult, state: Option<&BashState>) -> ToolContent {
        // Could include structured data, truncated output, etc.
        // Default is fine for most cases.
        result.result().clone()
    }
}
```


## What Sending Full State Costs

The mailbox replaces the value each time. For bash with 1000 lines
of output, each send copies the full `BashState` including the
`Vec<String>`. That's `O(n)` per send where n is accumulated lines.

Mitigation options:
1. **Send less often.** Batch: only send every 10 lines, or every
   100ms. The TUI renders at ~80ms anyway.
2. **Use `Rc<Vec<String>>` inside the state.** Clone is O(1).
3. **Accept it.** For most tools, state is small. Optimize later.

Recommendation: start with option 3.


## Summary

One trait: `Tool`. Associated type `State` (defaults to `String`).

Three execution tiers:
- **Sync**: return `ToolResult`, auto-converts to `ToolExecution` via `From`
- **Running**: `ToolExecution::running(future)` — async, default rendering
- **Custom**: `ToolExecution::custom(|sender| future)` — typed state via mailbox

`ToolExecution` enum: `Ready(ToolResult)` | `Running(future)` | `Custom { task, mailbox }`

Mailbox: single-slot latest-value. `Cell<Option<T>>`. Zero allocation per send.

Type erasure: `ErasedTool` wraps `Tool` via `Rc<T>`, `LiveOutputImpl<T>`
wraps mailbox + state + tool Rc behind `dyn LiveOutput`.

No boxing in the rendering path:
- `LiveOutput` methods take `&mut self` — no RefCell
- `LiveOutputImpl` stores `Rc<T>` — no boxed closures
- `latest` is `Option<T::State>` — plain field

LLM output: `Tool::to_result(&result, state)` controls what gets sent
back. Default: pass through the result's ToolContent. Override for
structured data, truncation, or enrichment.

Custom renderers own the entire content pane (title, content, everything).
Default renderers handle simple/async tools automatically.
