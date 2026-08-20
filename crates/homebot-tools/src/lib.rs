//! Server-enforced local computer capabilities for `HomeBot`.
//!
//! Filesystem, terminal and browser services in this crate cannot execute an
//! operation without first obtaining an unforgeable authorization value from
//! the server-owned policy engine.

mod activity;
mod browser;
mod contracts;
mod filesystem;
mod policy;
mod terminal;

pub use activity::{ActivitySink, NoopActivitySink, RecordingActivitySink};
pub use browser::{BrowserAction, BrowserResult, BrowserService, BrowserSessionProfile};
pub use contracts::{
    ActivityKind, ActivityStatus, CapabilityClass, CapabilityRequest, OperationContext,
    ToolActivity, ToolError,
};
pub use filesystem::{DirectoryEntry, FilesystemLimits, ScopedFilesystem};
pub use policy::{ApprovalDecision, ApprovalTicket, PolicyEffect, PolicyEngine, PolicyRule};
pub use terminal::{TerminalChunk, TerminalCommand, TerminalLimits, TerminalRun, TerminalService};
