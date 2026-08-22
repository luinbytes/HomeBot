//! Provider-neutral runtime boundary. Provider-native payloads terminate here.

mod bounded_io;
mod claude;
mod codex;
mod contracts;
mod discovery;
mod generic_process;
mod openai_compatible;
mod runtime;
mod secrets;
mod supervisor;

pub use claude::*;
pub use codex::*;
pub use contracts::*;
pub use generic_process::*;
pub use openai_compatible::*;
pub use runtime::*;
pub use secrets::*;
pub use supervisor::*;

#[cfg(test)]
mod tests;
