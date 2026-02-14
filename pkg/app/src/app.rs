//! Application scaffold — orchestrates sessions, commands, and input routing.

use mage_core::agent_loop::LoopError;
use mage_core::event_stream::AgentEventReceiver;
use mage_core::extension::{Disposition, ModelSelectArgs};
use mage_core::session::{AgentSession, SessionHandle};

use crate::command::{CommandError, CommandRegistry};

/// The top-level application. Owns the session, command registry,
/// and orchestrates input routing.
pub struct App {
    /// The current agent session.
    pub session: AgentSession,
    /// Registered slash commands.
    pub commands: CommandRegistry,
}

impl App {
    /// Create a new App wrapping an existing session.
    pub fn new(session: AgentSession) -> Self {
        Self {
            session,
            commands: CommandRegistry::new(),
        }
    }

    /// Get a cheap handle for external async code.
    pub fn handle(&self) -> SessionHandle {
        self.session.handle()
    }

    // UNRESOLVED: How do extensions register commands?
    //
    // The Extension trait lives in mage-core. CommandRegistry lives in mage-app.
    // Core cannot depend on app (circular). Options:
    //   (a) Add a generic callback slot to Extension::init (e.g. a trait object
    //       the app passes in alongside Registry)
    //   (b) A second init pass using a separate trait that mage-app defines
    //   (c) Extensions register commands via a well-known tool or convention
    //   (d) The application code registers commands explicitly, not via extensions
    //
    // For now, commands are registered by the application code that constructs
    // the App (e.g. `app.commands.register(...)`). Extension-driven command
    // registration is deferred until we resolve the layering.

    /// Process user input. Routes /commands to the command registry,
    /// everything else to the agent as a prompt.
    ///
    /// Returns:
    /// - `Ok(Some(rx))` — input sent to agent, events streaming
    /// - `Ok(None)` — input handled by command or hook (no agent invocation)
    /// - `Err(e)` — command or loop error
    pub async fn handle_input(
        &mut self,
        input: &str,
    ) -> Result<Option<AgentEventReceiver>, AppError> {
        let trimmed = input.trim();

        // Route slash commands
        if let Some(rest) = trimmed.strip_prefix('/') {
            let (cmd_name, cmd_args) = match rest.split_once(' ') {
                Some((name, args)) => (name, args.trim()),
                None => (rest, ""),
            };
            self.commands
                .execute(cmd_name, cmd_args, &mut self.session)
                .await
                .map_err(AppError::Command)?;
            return Ok(None);
        }

        // Fire on_input hooks — extensions can intercept/amend
        let final_text = {
            let mut text = trimmed.to_string();
            let mut handled = false;
            let mut exts = std::mem::take(&mut self.session.exts);
            for ext in exts.iter_mut() {
                let disp = ext.on_input(&text, &mut self.session).await;
                match disp {
                    Disposition::Propagate => {}
                    Disposition::Block { .. } => {
                        handled = true;
                        break;
                    }
                    Disposition::Value(amend) => {
                        text = amend.text;
                        if amend.handled {
                            handled = true;
                            break;
                        }
                    }
                }
            }
            self.session.exts = exts;
            if handled {
                return Ok(None);
            }
            text
        };

        // Send to agent
        let rx = self.session.prompt(&final_text).await
            .map_err(AppError::Loop)?;
        Ok(Some(rx))
    }

    /// Fire session start hooks.
    pub fn fire_session_start(&mut self) {
        let mut exts = std::mem::take(&mut self.session.exts);
        for ext in exts.iter_mut() {
            ext.on_session_start(&mut self.session);
        }
        self.session.exts = exts;
    }

    /// Fire session shutdown hooks.
    pub fn fire_session_shutdown(&mut self) {
        let mut exts = std::mem::take(&mut self.session.exts);
        for ext in exts.iter_mut() {
            ext.on_session_shutdown(&mut self.session);
        }
        self.session.exts = exts;
    }

    /// Fire model select hooks.
    pub fn fire_model_select(&mut self, model: &llm::Model) {
        let args = ModelSelectArgs { model };
        let mut exts = std::mem::take(&mut self.session.exts);
        for ext in exts.iter_mut() {
            ext.on_model_select(&args, &mut self.session);
        }
        self.session.exts = exts;
    }

    /// Access the underlying session.
    pub fn session(&self) -> &AgentSession {
        &self.session
    }

    /// Mutable access to the underlying session.
    pub fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }
}

/// Application-level error.
#[derive(Debug)]
pub enum AppError {
    Loop(LoopError),
    Command(CommandError),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loop(e) => write!(f, "{e}"),
            Self::Command(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AppError {}
