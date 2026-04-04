//! Built-in tools for the Mage coding agent.
//!
//! Provides the core tool set: Read, Edit, Write, Bash, Glob, Grep.
//! Each tool is a [`Module`] that registers one [`ToolHandler`].
//!
//! # Usage
//!
//! ```ignore
//! let modules = mage_tools::all();
//! // or pick individual tools:
//! let modules = vec![mage_tools::read(), mage_tools::bash()];
//! ```

use std::rc::Rc;
use mage_core::module::Module;

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod read;
pub mod recompile;
pub mod write;

/// All built-in tool modules.
pub fn all() -> Vec<Rc<dyn Module>> {
    vec![read(), edit(), write(), bash(), glob(), grep(), recompile()]
}

pub fn read() -> Rc<dyn Module> { Rc::new(read::ReadModule) }
pub fn edit() -> Rc<dyn Module> { Rc::new(edit::EditModule) }
pub fn write() -> Rc<dyn Module> { Rc::new(write::WriteModule) }
pub fn bash() -> Rc<dyn Module> { Rc::new(bash::BashModule) }
pub fn glob() -> Rc<dyn Module> { Rc::new(glob::GlobModule) }
pub fn grep() -> Rc<dyn Module> { Rc::new(grep::GrepModule) }
pub fn recompile() -> Rc<dyn Module> { Rc::new(recompile::RecompileModule) }
