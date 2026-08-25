use super::*;

#[test]
fn fixture_stream_normalizes_session_text_tools_usage_and_terminal()
-> Result<(), Box<dyn std::error::Error>> {
    let mut events = Vec::new();
    for line in include_str!("../../tests/fixtures/claude/turn.jsonl").lines() {
        let value: Value = serde_json::from_str(line)?;
        events.extend(normalize_message(&value, None));
    }
    assert!(
        matches!(events.first(), Some(ProviderEvent::ConversationStarted { conversation_id }) if conversation_id == "session_fixture")
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::ContentDelta { text } if text == "Hello"))
    );
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Activity { activity } if activity.kind == crate::ActivityKind::Terminal)));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Usage { usage } if usage.input_tokens == 10 && usage.output_tokens == 3)));
    assert_eq!(events.last(), Some(&ProviderEvent::Completed));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn fake_cli_streams_a_complete_turn() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir()?;
    let binary = directory.path().join("claude-fixture");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
IFS= read -r input
printf '%s\n' '{"type":"system","subtype":"init","session_id":"session_e2e"}'
printf '%s\n' '{"type":"stream_event","session_id":"session_e2e","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"session_e2e","usage":{"input_tokens":2,"output_tokens":1}}'
"#,
    )?;
    let mut permissions = std::fs::metadata(&binary)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&binary, permissions)?;
    let adapter = ClaudeAdapter::new(ClaudeProfile::new(
        ProviderAdapterId::new("claude-fixture")?,
        &binary,
    ));
    let mut run = adapter
        .start(StartRequest {
            operation_id: Uuid::now_v7(),
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            prompt: "Hello".to_owned(),
            model: Some("sonnet".to_owned()),
            working_directory: None,
            mode: ExecutionMode::Normal,
            attachments: Vec::new(),
        })
        .await?;
    assert!(
        matches!(run.events.recv().await, Some(ProviderEvent::ConversationStarted { conversation_id }) if conversation_id == "session_e2e")
    );
    assert_eq!(
        run.events.recv().await,
        Some(ProviderEvent::ContentDelta {
            text: "Hi".to_owned()
        })
    );
    assert!(matches!(
        run.events.recv().await,
        Some(ProviderEvent::Usage { .. })
    ));
    assert_eq!(run.events.recv().await, Some(ProviderEvent::Completed));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_stops_the_cli_and_emits_cancelled() -> Result<(), Box<dyn std::error::Error>>
{
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir()?;
    let binary = directory.path().join("claude-cancel-fixture");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
IFS= read -r input
printf '%s\n' '{"type":"system","subtype":"init","session_id":"session_cancel"}'
while IFS= read -r more; do :; done
"#,
    )?;
    let mut permissions = std::fs::metadata(&binary)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&binary, permissions)?;
    let adapter = ClaudeAdapter::new(ClaudeProfile::new(
        ProviderAdapterId::new("claude-cancel")?,
        &binary,
    ));
    let operation_id = Uuid::now_v7();
    let mut run = adapter
        .start(StartRequest {
            operation_id,
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            prompt: "Wait".to_owned(),
            model: None,
            working_directory: None,
            mode: ExecutionMode::Normal,
            attachments: Vec::new(),
        })
        .await?;
    assert!(matches!(
        run.events.recv().await,
        Some(ProviderEvent::ConversationStarted { .. })
    ));
    adapter.cancel(operation_id).await?;
    assert_eq!(run.events.recv().await, Some(ProviderEvent::Cancelled));
    Ok(())
}

#[tokio::test]
async fn smoke_test_skips_when_claude_is_not_installed() -> Result<(), Box<dyn std::error::Error>> {
    let profile = ClaudeProfile::new(ProviderAdapterId::new("claude-smoke")?, "claude");
    if resolve_binary(&profile).is_none() {
        eprintln!("SKIP: claude executable is not installed");
        return Ok(());
    }
    let adapter = ClaudeAdapter::new(profile);
    assert_ne!(
        adapter.health().await.availability,
        ProviderAvailability::NotInstalled
    );
    Ok(())
}
