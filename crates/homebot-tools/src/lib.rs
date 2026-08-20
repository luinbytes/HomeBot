//! Server-enforced filesystem, terminal, browser, and plugin capability boundary.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityClass {
    FilesystemRead,
    FilesystemWrite,
    ProcessExecute,
    BrowserObserve,
    BrowserAct,
    ExternalMutation,
    SecretUse,
}
