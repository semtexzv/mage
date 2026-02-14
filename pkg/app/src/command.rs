use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use refstr::Str;

use mage_core::session::AgentSession;

/// Async command handler. Receives the argument string and a session handle.
pub type CommandHandler = Rc<
    dyn Fn(&str, &mut AgentSession) -> Pin<Box<dyn Future<Output = Result<(), CommandError>>>>,
>;

/// A slash command registered by an extension or the application.
pub struct Command {
    pub name: Str,
    pub description: Option<Str>,
    pub handler: CommandHandler,
}

#[derive(Debug)]
pub enum CommandError {
    /// Command not found.
    NotFound(Str),
    /// Agent is not idle — cannot execute command now.
    NotIdle,
    /// Generic error.
    Other(String),
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "command not found: /{name}"),
            Self::NotIdle => write!(f, "agent is busy"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Registry of slash commands. Built during app initialization.
pub struct CommandRegistry {
    commands: HashMap<Str, Command>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self { commands: HashMap::new() }
    }

    /// Register a command.
    pub fn register(&mut self, cmd: Command) {
        self.commands.insert(cmd.name.clone(), cmd);
    }

    /// Look up a command by name.
    pub fn get(&self, name: &str) -> Option<&Command> {
        self.commands.get(name)
    }

    /// Execute a command by name with the given args string.
    pub async fn execute(
        &self,
        name: &str,
        args: &str,
        session: &mut AgentSession,
    ) -> Result<(), CommandError> {
        let cmd = self.commands.get(name)
            .ok_or_else(|| CommandError::NotFound(name.into()))?;
        (cmd.handler)(args, session).await
    }

    /// List all registered command names.
    pub fn names(&self) -> Vec<&Str> {
        self.commands.keys().collect()
    }

    /// Number of registered commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}
