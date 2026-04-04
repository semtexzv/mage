//! App trait and TUI runtime with tokio event loop.
//!
//! Runs in the **main terminal** (no alternate screen). Scrollback is
//! preserved — content scrolls up naturally as it grows.

use crossterm::event::{
    Event as CtEvent, EventStream, KeyEvent, KeyEventKind,
    KeyboardEnhancementFlags, PushKeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
};
use crossterm::terminal;
use futures::StreamExt;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;
use tokio::time::{self, Sleep};

use crate::renderer::{ProcessTerminal, Renderer, Terminal};

/// Events delivered to the app.
pub enum Event<M: 'static> {
    Key(KeyEvent),
    Resize(u16, u16),
    Message(M),
    /// Bracketed paste — content pasted from the clipboard.
    Paste(String),
}

/// The application trait.
///
/// No `Send` bound — both the app and message producers run on the main
/// thread via `spawn_local`. Nothing crosses thread boundaries.
pub trait App: 'static {
    type Message: 'static;
    /// Render into the renderer. Push lines, render views, set cursor.
    fn render(&mut self, r: &mut Renderer);
    /// Handle an event. Return `true` to quit.
    fn update(&mut self, event: Event<Self::Message>) -> bool;
}

// ── Terminal guard ──────────────────────────────────────────────

/// Global flag so the panic hook knows raw mode is active.
static RAW_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Restores terminal state: show cursor, disable raw mode, print newline.
///
/// Safe to call multiple times (idempotent via atomic flag).
/// Call before `process::exit()` to ensure the terminal is usable.
pub fn restore_terminal() {
    if RAW_MODE_ACTIVE.swap(false, Ordering::SeqCst) {
        // Best-effort — ignore errors, we may be in a panic/signal handler.
        // Re-enable auto-wrap (DECAWM on) before restoring normal mode.
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::EnableLineWrap);
        let _ = crossterm::execute!(
            std::io::stdout(),
            PopKeyboardEnhancementFlags,
            crossterm::event::DisableBracketedPaste,
            crossterm::cursor::Show
        );
        let _ = terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::style::Print("\n"));
    }
}

/// RAII guard that restores terminal on drop (normal exit, early return, panic unwind).
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Install a panic hook that restores the terminal before printing the panic.
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        original(info);
    }));
}

// ── Runtime ─────────────────────────────────────────────────────

/// Run an `App` on a real terminal with tokio (no async messages).
pub async fn run<A: App>(app: A) -> A {
    let (_tx, rx) = mpsc::channel::<A::Message>(1);
    run_with_messages(app, rx).await
}

/// Run an app with an async message channel.
///
/// ```ignore
/// let (tx, rx) = tokio::sync::mpsc::channel(256);
/// tokio::spawn(async move { tx.send(MyMsg::Tick).await; });
/// run_with_messages(my_app, rx).await;
/// ```
pub async fn run_with_messages<A: App>(mut app: A, mut msg_rx: mpsc::Receiver<A::Message>) -> A {
    install_panic_hook();

    terminal::enable_raw_mode().expect("enable raw mode");
    RAW_MODE_ACTIVE.store(true, Ordering::SeqCst);
    let _guard = RawModeGuard;

    crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableBracketedPaste,
        // Enable kitty keyboard protocol — lets us detect Shift+Enter,
        // Ctrl+Enter, etc. Silently ignored by terminals that don't
        // support it; the backslash workaround still works as fallback.
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        ),
        terminal::Clear(terminal::ClearType::Purge),
        crossterm::cursor::MoveTo(0, 0),
    )
    .ok();

    // Disable auto-wrap (DECAWM off): characters past the right margin
    // are clipped instead of wrapping to the next line.
    crossterm::execute!(std::io::stdout(), crossterm::terminal::DisableLineWrap).ok();

    let mut term = ProcessTerminal::new();
    let mut renderer = Renderer::new();

    let do_render = |app: &mut A, term: &mut ProcessTerminal, ren: &mut Renderer| {
        let (w, h) = term.size();
        ren.begin_frame(w, h);
        app.render(ren);
        ren.end_frame(term);
    };
    do_render(&mut app, &mut term, &mut renderer);
    if term.has_error() {
        renderer.finalize(&mut term);
        return app;
    }

    let mut ct_stream = EventStream::new();

    // Resize debounce: collect rapid resize events and render once at
    // the final size. The timer is reset on each new resize event.
    const RESIZE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(30);
    let mut resize_pending = false;
    let mut resize_timer: Pin<Box<Sleep>> = Box::pin(time::sleep(RESIZE_DEBOUNCE));
    // Start the timer in a "completed" state so it doesn't fire spuriously.
    // We'll reset it when the first resize arrives.
    resize_timer.as_mut().reset(time::Instant::now());

    loop {
        let quit = tokio::select! {
            // Bias toward draining crossterm events before firing the timer,
            // so back-to-back resizes keep resetting the debounce.
            biased;

            ct = ct_stream.next() => {
                match ct {
                    Some(Ok(CtEvent::Key(key))) => {
                        if key.kind != KeyEventKind::Press { continue; }
                        app.update(Event::Key(key))
                    }
                    Some(Ok(CtEvent::Paste(text))) => {
                        app.update(Event::Paste(text))
                    }
                    Some(Ok(CtEvent::Resize(_w, _h))) => {
                        term.update_size();
                        // Don't render now — (re)start the debounce timer.
                        resize_pending = true;
                        resize_timer.as_mut().reset(time::Instant::now() + RESIZE_DEBOUNCE);
                        continue;
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => true,
                    None => true,
                }
            }
            _ = &mut resize_timer, if resize_pending => {
                // Debounce timer fired — read final size and render once.
                resize_pending = false;
                term.update_size();
                let (w, h) = term.size();
                app.update(Event::Resize(w, h))
            }
            msg = msg_rx.recv() => {
                match msg {
                    Some(m) => app.update(Event::Message(m)),
                    None => true,
                }
            }
        };

        if quit {
            break;
        }

        do_render(&mut app, &mut term, &mut renderer);
        if term.has_error() {
            break;
        }
    }

    // Move cursor past content before guard restores terminal.
    renderer.finalize(&mut term);

    // _guard drops here → restore_terminal()
    app
}
