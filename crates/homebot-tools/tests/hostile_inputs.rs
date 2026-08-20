use homebot_tools::{
    BrowserService, CapabilityClass, FilesystemLimits, OperationContext, PolicyEffect,
    PolicyEngine, PolicyRule, RecordingActivitySink, ScopedFilesystem, TerminalCommand,
    TerminalLimits, TerminalService, ToolError,
};
use reqwest::Url;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};
use uuid::Uuid;

fn context() -> OperationContext {
    OperationContext {
        operation_id: Uuid::now_v7(),
        owner_id: Uuid::now_v7(),
        device_id: Uuid::now_v7(),
        bot_id: Uuid::now_v7(),
        chat_id: Uuid::now_v7(),
        workspace_id: Uuid::now_v7(),
    }
}

#[tokio::test]
async fn hostile_paths_and_unapproved_mutations_fail_before_side_effects()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    std::fs::write(outside.path().join("credential"), "canary")?;
    let activities = Arc::new(RecordingActivitySink::default());
    let policy = Arc::new(PolicyEngine::new(
        Duration::from_secs(60),
        activities.clone(),
    ));
    let filesystem = ScopedFilesystem::new(
        workspace.path(),
        policy,
        activities,
        FilesystemLimits::default(),
    )?;
    assert_eq!(
        filesystem.read(context(), "../credential", None).await,
        Err(ToolError::PathOutsideWorkspace)
    );
    assert!(matches!(
        filesystem
            .write(context(), "blocked", b"payload".to_vec(), None)
            .await,
        Err(ToolError::ApprovalRequired(_))
    ));
    assert!(!workspace.path().join("blocked").exists());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("escape"))?;
        assert_eq!(
            filesystem.read(context(), "escape/credential", None).await,
            Err(ToolError::SymlinkRejected)
        );
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn hostile_terminal_environment_and_working_directory_are_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let activities = Arc::new(RecordingActivitySink::default());
    let policy = Arc::new(PolicyEngine::new(
        Duration::from_secs(60),
        activities.clone(),
    ));
    policy
        .replace_rules(vec![PolicyRule::new(
            CapabilityClass::ProcessExecute,
            PolicyEffect::Allow,
        )])
        .await;
    let terminal = TerminalService::new(
        workspace.path(),
        policy,
        activities,
        TerminalLimits::default(),
    )?;
    let base = TerminalCommand {
        program: PathBuf::from("/bin/sh"),
        arguments: vec!["-c".to_owned(), "printf safe".to_owned()],
        working_directory: PathBuf::new(),
        environment: BTreeMap::new(),
        rows: 24,
        columns: 80,
    };
    let mut poisoned = base.clone();
    poisoned
        .environment
        .insert("LD_PRELOAD".to_owned(), "/tmp/evil.so".to_owned());
    assert!(matches!(
        terminal.start(context(), poisoned, None).await,
        Err(ToolError::InvalidRequest(_))
    ));
    let mut traversal = base;
    traversal.working_directory = PathBuf::from("../outside");
    assert!(matches!(
        terminal.start(context(), traversal, None).await,
        Err(ToolError::PathOutsideWorkspace)
    ));
    Ok(())
}

#[test]
fn remote_browser_control_endpoint_is_never_accepted() -> Result<(), Box<dyn std::error::Error>> {
    let profiles = tempfile::tempdir()?;
    let activities = Arc::new(RecordingActivitySink::default());
    let policy = Arc::new(PolicyEngine::new(
        Duration::from_secs(60),
        activities.clone(),
    ));
    assert!(matches!(
        BrowserService::new(
            Url::parse("http://192.0.2.10:9222/")?,
            profiles.path(),
            policy,
            activities,
        ),
        Err(ToolError::InvalidRequest(_))
    ));
    Ok(())
}
