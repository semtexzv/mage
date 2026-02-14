//! Agent construction — builder and init recipe for creating sessions.
//!
//! ## Construction
//!
//! Use [`AgentBuilder`] to configure, then either:
//! - `.build()` for a single session (convenience)
//! - `.into_init()` for a clonable [`AgentInit`] recipe, then `.spawn()` per session
//!
//! ```ignore
//! let init = AgentBuilder::new(model)
//!     .system_prompt("You are helpful.")
//!     .provider("anthropic", AnthropicProvider::new())
//!     .ext_factory(|| Box::new(MyExtension::new()))
//!     .into_init();
//!
//! let mut session = init.spawn();       // main session
//! let mut sub     = init.spawn();       // sub-agent: fresh extensions, empty history
//! ```

use std::rc::Rc;

use refstr::Str;
use llm::{Message, Model, Provider, StreamOptions};

use crate::agent_loop::{LoopConfig, StreamFn};
use crate::extension::{Extension, ExtensionFactory};
use crate::session::AgentSession;
use crate::types::{AgentMessage, AgentState};

// ---------------------------------------------------------------------------
// AgentInit — clonable recipe for spawning sessions
// ---------------------------------------------------------------------------

/// Clonable recipe for spawning sessions. Cheap to clone (Rc internals).
///
/// Each [`spawn()`] call produces a fresh `AgentSession` with:
/// - Fresh extension instances (from factories)
/// - Empty message history
/// - Own cancel token
/// - Shared providers (Rc — same HTTP client, connection pooling)
///
/// Tools are NOT stored here. They come from `Extension::init()` which
/// runs inside the agent loop. Each agent gets its own tools from its
/// own extension instances.
#[derive(Clone)]
pub struct AgentInit {
    pub model: Model,
    pub system_prompt: String,
    pub options: StreamOptions,
    pub max_turns: u32,
    /// Providers shared across all sessions spawned from this init.
    /// `Rc<dyn Provider>` because providers are stateless (HTTP client + key).
    providers: Rc<Vec<(Str, Rc<dyn Provider>)>>,
    /// Extension factories — called per spawn to get fresh instances.
    ext_factories: Rc<Vec<ExtensionFactory>>,
    /// Custom stream function override. If set, bypasses provider resolution.
    stream_fn_override: Option<Rc<StreamFnShared>>,
    /// Message conversion function.
    convert_to_llm: Rc<dyn Fn(&[AgentMessage]) -> Vec<Message>>,
}

/// Shared stream function type (Rc-wrapped for clonability).
type StreamFnShared = dyn Fn(llm::StreamRequest) -> llm::StreamHandle;

impl AgentInit {
    /// Spawn a fresh session. Fresh extensions, empty history, own cancel token.
    ///
    /// Tools and extension-provided providers are registered later when
    /// `Extension::init()` runs inside the agent loop.
    pub fn spawn(&self) -> AgentSession {
        let exts: Vec<Box<dyn Extension>> =
            self.ext_factories.iter().map(|f| f()).collect();

        let stream_fn = self.build_stream_fn();

        let convert_rc = self.convert_to_llm.clone();
        let convert_to_llm: Box<dyn Fn(&[AgentMessage]) -> Vec<Message>> =
            Box::new(move |msgs| convert_rc(msgs));

        let state = AgentState {
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            messages: Vec::new(),
            tools: Vec::new(), // populated by Extension::init in the loop
            options: self.options.clone(),
        };

        let config = LoopConfig {
            max_turns: self.max_turns,
            stream_fn,
            options: self.options.clone(),
            convert_to_llm,
        };

        AgentSession::from_parts(state, exts, config)
    }

    /// Spawn with overrides applied to a clone of this init.
    pub fn spawn_with(&self, f: impl FnOnce(&mut AgentInit)) -> AgentSession {
        let mut init = self.clone();
        f(&mut init);
        init.spawn()
    }

    fn build_stream_fn(&self) -> StreamFn {
        if let Some(shared) = &self.stream_fn_override {
            let shared = shared.clone();
            return Box::new(move |req| shared(req));
        }
        let providers = self.providers.clone();
        let model = self.model.clone();
        Box::new(move |req| {
            let provider = providers.iter()
                .find(|(api, _)| **api == *model.api)
                .map(|(_, p)| p.clone());
            match provider {
                Some(p) => p.stream(req),
                None => {
                    let api = model.api.to_string();
                    let (_, rx) = llm::channel::channel();
                    llm::StreamHandle {
                        events: rx,
                        task: Box::pin(async move {
                            Err(llm::ProviderError::Other(
                                format!("no provider registered for api: {api}"),
                            ))
                        }),
                    }
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// AgentBuilder
// ---------------------------------------------------------------------------

pub struct AgentBuilder {
    system_prompt: String,
    model: Model,
    providers: Vec<(Str, Rc<dyn Provider>)>,
    stream_fn: Option<StreamFn>,
    exts: Vec<Box<dyn Extension>>,
    ext_factories: Vec<ExtensionFactory>,
    options: StreamOptions,
    max_turns: u32,
    convert_to_llm: Option<Rc<dyn Fn(&[AgentMessage]) -> Vec<Message>>>,
}

impl AgentBuilder {
    pub fn new(model: Model) -> Self {
        Self {
            system_prompt: String::new(),
            model,
            providers: Vec::new(),
            stream_fn: None,
            exts: Vec::new(),
            ext_factories: Vec::new(),
            options: StreamOptions::default(),
            max_turns: 0,
            convert_to_llm: None,
        }
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Register a provider. Stored as `Rc<dyn Provider>` — shared across
    /// all sessions spawned from the same `AgentInit`.
    pub fn provider(mut self, api: impl Into<Str>, provider: impl Provider + 'static) -> Self {
        self.providers.push((api.into(), Rc::new(provider)));
        self
    }

    /// Set a stream function override. Bypasses provider resolution.
    /// Not clonable — only usable with `.build()`, not `.into_init().spawn()`.
    pub fn stream_fn(mut self, f: StreamFn) -> Self {
        self.stream_fn = Some(f);
        self
    }

    /// Add an already-constructed extension instance.
    ///
    /// This instance is consumed by the first session. For sub-agent support,
    /// use `.ext_factory()` instead — it creates fresh instances per spawn.
    pub fn ext(mut self, ext: impl Extension + 'static) -> Self {
        self.exts.push(Box::new(ext));
        self
    }

    /// Register an extension factory. Called once per `spawn()` to produce
    /// a fresh extension instance. This is the sub-agent-safe path.
    pub fn ext_factory(mut self, f: impl Fn() -> Box<dyn Extension> + 'static) -> Self {
        self.ext_factories.push(Rc::new(f));
        self
    }

    /// Add all extension factories from a `FactoryRegistry`.
    pub fn ext_from_registry(mut self, registry: &crate::extension::FactoryRegistry) -> Self {
        self.ext_factories.extend(registry.clone_factories());
        self
    }

    pub fn options(mut self, options: StreamOptions) -> Self {
        self.options = options;
        self
    }

    pub fn max_turns(mut self, n: u32) -> Self {
        self.max_turns = n;
        self
    }

    pub fn convert_to_llm(
        mut self,
        f: impl Fn(&[AgentMessage]) -> Vec<Message> + 'static,
    ) -> Self {
        self.convert_to_llm = Some(Rc::new(f));
        self
    }

    /// Produce a clonable `AgentInit` recipe.
    ///
    /// One-shot extensions from `.ext()` become single-use factories:
    /// first `spawn()` gets the real instance, subsequent spawns get a no-op.
    /// Use `.ext_factory()` for proper sub-agent support.
    pub fn into_init(self) -> AgentInit {
        let mut ext_factories = self.ext_factories;

        // Wrap one-shot extensions as single-use factories.
        for ext in self.exts {
            let cell = std::cell::RefCell::new(Some(ext));
            ext_factories.push(Rc::new(move || {
                cell.borrow_mut().take().unwrap_or_else(|| Box::new(NoopExtension))
            }));
        }

        let convert_to_llm = self.convert_to_llm
            .unwrap_or_else(|| Rc::new(default_convert_to_llm));

        // Wrap non-clonable stream_fn as Rc for AgentInit.
        let stream_fn_override = self.stream_fn.map(|f| {
            let f = Rc::new(f);
            Rc::new(move |req: llm::StreamRequest| {
                f(req)
            }) as Rc<StreamFnShared>
        });

        AgentInit {
            model: self.model,
            system_prompt: self.system_prompt,
            options: self.options,
            max_turns: self.max_turns,
            providers: Rc::new(self.providers),
            ext_factories: Rc::new(ext_factories),
            stream_fn_override,
            convert_to_llm,
        }
    }

    /// Convenience: build and spawn a single session in one step.
    pub fn build(self) -> AgentSession {
        self.into_init().spawn()
    }
}

// ---------------------------------------------------------------------------
// NoopExtension — placeholder for consumed one-shot extensions
// ---------------------------------------------------------------------------

struct NoopExtension;
impl Extension for NoopExtension {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Default conversion: extract LLM messages, drop custom messages.
fn default_convert_to_llm(messages: &[AgentMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|m| match m {
            AgentMessage::Llm(msg) => Some(msg.clone()),
            AgentMessage::Custom { .. } => None,
        })
        .collect()
}
