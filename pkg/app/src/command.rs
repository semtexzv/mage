use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use refstr::Str;

use mage_core::session::SessionHandle;

/// Async command handler. Receives the argument string and a session handle.
///
/// Commands interact with the session exclusively through the handle:
/// inject messages, abort, check idle status.  They never access
/// session state, tools, or modules directly.
pub type CommandHandler = Rc<
    dyn Fn(&str, &SessionHandle) -> Pin<Box<dyn Future<Output = Result<(), CommandError>>>>,
>;

/// A slash command registered by a module or the application.
#[derive(Clone)]
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
#[derive(Clone)]
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
        handle: &SessionHandle,
    ) -> Result<(), CommandError> {
        let cmd = self.commands.get(name)
            .ok_or_else(|| CommandError::NotFound(name.into()))?;
        (cmd.handler)(args, handle).await
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_handler() -> CommandHandler {
        Rc::new(|_args: &str, _handle: &SessionHandle| {
            Box::pin(async { Ok(()) })
        })
    }

    fn failing_handler(msg: &'static str) -> CommandHandler {
        Rc::new(move |_args: &str, _handle: &SessionHandle| {
            Box::pin(async move { Err(CommandError::Other(msg.to_string())) })
        })
    }

    #[test]
    fn register_and_get() {
        let mut reg = CommandRegistry::new();
        reg.register(Command {
            name: "help".into(),
            description: Some("Show help".into()),
            handler: dummy_handler(),
        });
        assert!(reg.get("help").is_some());
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn names_returns_all_registered() {
        let mut reg = CommandRegistry::new();
        reg.register(Command { name: "alpha".into(), description: None, handler: dummy_handler() });
        reg.register(Command { name: "beta".into(), description: None, handler: dummy_handler() });
        let mut names: Vec<&str> = reg.names().into_iter().map(|s| &**s).collect();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn execute_not_found() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            let local = tokio::task::LocalSet::new();
            local.run_until(async {
                let reg = CommandRegistry::new();
                let handle = SessionHandle::test_handle();
                let result = reg.execute("nope", "", &handle).await;
                assert!(matches!(result, Err(CommandError::NotFound(_))));
            }).await;
        });
    }

    #[test]
    fn execute_calls_handler() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            let local = tokio::task::LocalSet::new();
            local.run_until(async {
                let mut reg = CommandRegistry::new();
                reg.register(Command { name: "ping".into(), description: None, handler: dummy_handler() });
                let handle = SessionHandle::test_handle();
                let result = reg.execute("ping", "arg1", &handle).await;
                assert!(result.is_ok());
            }).await;
        });
    }

    #[test]
    fn execute_propagates_error() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            let local = tokio::task::LocalSet::new();
            local.run_until(async {
                let mut reg = CommandRegistry::new();
                reg.register(Command { name: "fail".into(), description: None, handler: failing_handler("boom") });
                let handle = SessionHandle::test_handle();
                let result = reg.execute("fail", "", &handle).await;
                assert!(matches!(result, Err(CommandError::Other(msg)) if msg == "boom"));
            }).await;
        });
    }

    #[test]
    fn display_formats() {
        assert_eq!(CommandError::NotFound("x".into()).to_string(), "command not found: /x");
        assert_eq!(CommandError::NotIdle.to_string(), "agent is busy");
        assert_eq!(CommandError::Other("oops".into()).to_string(), "oops");
    }

    #[test]
    fn registry_len_and_is_empty() {
        let mut reg = CommandRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        reg.register(Command { name: "x".into(), description: None, handler: dummy_handler() });
        assert!(!reg.is_empty());
        assert_eq!(reg.len(), 1);
    }
}