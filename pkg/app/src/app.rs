//! Application scaffold — orchestrates commands and input routing.
//!
//! The `App` owns a [`SessionHandle`] and a [`CommandRegistry`].
//! It does **not** own the session — that lives inside the spawned
//! session loop task (see [`mage_core::session::spawn`]).

use mage_core::session::SessionHandle;

use crate::command::{CommandError, CommandRegistry};

/// The top-level application.  Holds a handle to the session and
/// the command registry.
pub struct App {
    /// Handle to the running session.
    pub handle: SessionHandle,
    /// Registered slash commands.
    pub commands: CommandRegistry,
}

/// What happened with the user's input.
pub enum InputAction {
    /// Sent to the session as a user prompt.
    Submitted,
    /// Handled by a slash command (no agent invocation).
    Command,
}

impl App {
    /// Create a new App from a session handle.
    pub fn new(handle: SessionHandle) -> Self {
        Self {
            handle,
            commands: CommandRegistry::new(),
        }
    }

    // UNRESOLVED: How do extensions register commands?
    //
    // The Extension trait lives in mage-core. CommandRegistry lives in mage-app.
    // Core cannot depend on app (circular). Options:
    //   (a) Add a generic callback slot to Extension::init
    //   (b) A second init pass using a separate trait that mage-app defines
    //   (c) Extensions register commands via a well-known tool or convention
    //   (d) The application code registers commands explicitly
    //
    // For now, commands are registered by the application code that constructs
    // the App (e.g. `app.commands.register(...)`).

    /// Process user input.  Routes /commands to the command registry,
    /// everything else to the session as user input.
    pub async fn handle_input(
        &self,
        input: &str,
    ) -> Result<InputAction, AppError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(InputAction::Command); // nothing to do
        }

        // Route slash commands.
        if let Some(rest) = trimmed.strip_prefix('/') {
            let (cmd_name, cmd_args) = match rest.split_once(' ') {
                Some((name, args)) => (name, args.trim()),
                None => (rest, ""),
            };
            self.commands
                .execute(cmd_name, cmd_args, &self.handle)
                .await
                .map_err(AppError::Command)?;
            return Ok(InputAction::Command);
        }

        // Send to the session.
        self.handle.send_input(trimmed);
        Ok(InputAction::Submitted)
    }
}

/// Application-level error.
#[derive(Debug)]
pub enum AppError {
    Command(CommandError),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Command(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AppError {}
