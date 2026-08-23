use super::*;
use crate::ActivityKind;

#[test]
fn fixture_notifications_normalize_without_native_payloads()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = include_str!("../../tests/fixtures/codex/turn.jsonl");
    let activity_id = Uuid::now_v7();
    let mut events = Vec::new();
    for line in fixture.lines() {
        let message: Value = serde_json::from_str(line)?;
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").unwrap_or(&Value::Null);
        events.extend(notification_events(method, params, Some(activity_id), None));
    }
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::ContentDelta { text } if text == "Hello"))
    );
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Activity { activity } if activity.kind == ActivityKind::Terminal)));
    assert!(events.iter().any(|event| matches!(event, ProviderEvent::Usage { usage } if usage.input_tokens == 12 && usage.output_tokens == 4)));
    assert_eq!(events.last(), Some(&ProviderEvent::Completed));
    Ok(())
}

#[test]
fn native_errors_and_interruption_have_stable_normalized_results() {
    let error = normalize_codex_error(&json!({
        "error": {"message": "Sign in again", "codexErrorInfo": "Unauthorized"}
    }));
    assert_eq!(error.code, ProviderErrorCode::AuthenticationRequired);
    let events = notification_events(
        "turn/completed",
        &json!({"turn": {"status": "interrupted"}}),
        None,
        None,
    );
    assert_eq!(events, vec![ProviderEvent::Cancelled]);
}

#[test]
fn explicit_profiles_are_independent_and_do_not_debug_environment_values()
-> Result<(), Box<dyn std::error::Error>> {
    let first = CodexProfile::new(ProviderAdapterId::new("codex-work")?, "/opt/codex-work")
        .environment("CODEX_HOME", "/secret/work");
    let second = CodexProfile::new(
        ProviderAdapterId::new("codex-personal")?,
        "/opt/codex-personal",
    );
    assert_ne!(first.adapter_id, second.adapter_id);
    assert!(!format!("{:?}", CodexAdapter::new(first)).contains("/secret/work"));
    Ok(())
}

#[tokio::test]
async fn turn_fails_closed_without_an_effective_model() -> Result<(), Box<dyn std::error::Error>> {
    let adapter = CodexAdapter::new(CodexProfile::new(
        ProviderAdapterId::new("codex-plan")?,
        "missing-codex",
    ));
    let result = adapter
        .begin_turn(
            Uuid::now_v7(),
            "thread".to_owned(),
            "Plan".to_owned(),
            None,
            ExecutionMode::Plan,
        )
        .await;
    assert!(matches!(
        result,
        Err(ProviderError {
            code: ProviderErrorCode::ProtocolViolation,
            ..
        })
    ));
    Ok(())
}

#[tokio::test]
async fn smoke_test_skips_explicitly_when_codex_is_not_installed()
-> Result<(), Box<dyn std::error::Error>> {
    let profile = CodexProfile::new(ProviderAdapterId::new("codex-smoke")?, "codex");
    if resolve_binary(&profile).is_none() {
        eprintln!("SKIP: codex executable is not installed");
        return Ok(());
    }
    let adapter = CodexAdapter::new(profile);
    let descriptor = adapter.discover().await?;
    assert!(descriptor.executable.is_some());
    assert_ne!(
        adapter.health().await.availability,
        ProviderAvailability::NotInstalled
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn app_server_fixture_streams_and_resolves_approval() -> Result<(), Box<dyn std::error::Error>>
{
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir()?;
    let binary = directory.path().join("codex-fixture");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      case "$line" in
        *'"capabilities":{"experimentalApi":true}'*) ;;
        *) exit 2 ;;
      esac
      printf '%s\n' '{"id":1,"result":{"userAgent":"fixture","platformFamily":"unix","platformOs":"test"}}'
      ;;
    *'"method":"thread/start"'*)
      case "$line" in
        *'"approvalPolicy":"untrusted"'*'"sandbox":"read-only"'*) ;;
        *) exit 2 ;;
      esac
      printf '%s\n' '{"id":2,"result":{"thread":{"id":"thr_fixture"},"model":"fixture-model"}}'
      ;;
    *'"method":"turn/start"'*)
      case "$line" in
        *'"collaborationMode":{"mode":"default","settings":{"developer_instructions":null,"model":"fixture-model"}}'*) ;;
        *) exit 2 ;;
      esac
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"turn_fixture","status":"inProgress","items":[],"error":null}}}'
      printf '%s\n' '{"method":"item/commandExecution/requestApproval","id":900,"params":{"threadId":"thr_fixture","turnId":"turn_fixture","itemId":"item_fixture","command":["cargo","test"],"cwd":"/workspace","reason":"Run tests"}}'
      ;;
    *'"decision":"accept"'*)
      printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"thr_fixture","turnId":"turn_fixture","itemId":"message_fixture","delta":"Tests passed"}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thr_fixture","turn":{"id":"turn_fixture","status":"completed","items":[],"error":null}}}'
      exit 0
      ;;
  esac
done
"#,
    )?;
    let mut permissions = std::fs::metadata(&binary)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&binary, permissions)?;

    let adapter = CodexAdapter::new(CodexProfile::new(
        ProviderAdapterId::new("codex-fixture")?,
        &binary,
    ));
    let operation_id = Uuid::now_v7();
    let mut run = adapter
        .start(StartRequest {
            operation_id,
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            prompt: "Run tests".to_owned(),
            model: None,
            mode: ExecutionMode::Normal,
            attachments: Vec::new(),
        })
        .await?;
    assert_eq!(
        run.events.recv().await,
        Some(ProviderEvent::ConversationStarted {
            conversation_id: "thr_fixture".to_owned()
        })
    );
    let approval_id = match run.events.recv().await {
        Some(ProviderEvent::ApprovalRequired { approval }) => approval.approval_id,
        event => return Err(format!("expected approval, got {event:?}").into()),
    };
    adapter
        .resolve_approval(approval_id, ApprovalDecision::AllowOnce)
        .await?;
    assert_eq!(
        run.events.recv().await,
        Some(ProviderEvent::ContentDelta {
            text: "Tests passed".to_owned()
        })
    );
    assert_eq!(run.events.recv().await, Some(ProviderEvent::Completed));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn app_server_fixture_resumes_and_interrupts_a_turn() -> Result<(), Box<dyn std::error::Error>>
{
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir()?;
    let binary = directory.path().join("codex-interrupt-fixture");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      case "$line" in
        *'"capabilities":{"experimentalApi":true}'*) ;;
        *) exit 2 ;;
      esac
      printf '%s\n' '{"id":1,"result":{}}'
      ;;
    *'"method":"thread/resume"'*)
      case "$line" in
        *'"approvalPolicy":"untrusted"'*'"sandbox":"read-only"'*) ;;
        *) exit 2 ;;
      esac
      printf '%s\n' '{"id":2,"result":{"thread":{"id":"thr_existing"},"model":"fixture-model"}}'
      ;;
    *'"method":"turn/start"'*)
      case "$line" in
        *'"collaborationMode":{"mode":"plan","settings":{"developer_instructions":null,"model":"fixture-model"}}'*) ;;
        *) exit 2 ;;
      esac
      printf '%s\n' '{"id":3,"result":{"turn":{"id":"turn_interrupt","status":"inProgress","items":[],"error":null}}}'
      ;;
    *'"method":"turn/interrupt"'*)
      printf '%s\n' '{"id":4,"result":{}}'
      printf '%s\n' '{"method":"turn/completed","params":{"threadId":"thr_existing","turn":{"id":"turn_interrupt","status":"interrupted","items":[],"error":null}}}'
      exit 0
      ;;
  esac
done
"#,
    )?;
    let mut permissions = std::fs::metadata(&binary)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&binary, permissions)?;

    let adapter = CodexAdapter::new(CodexProfile::new(
        ProviderAdapterId::new("codex-interrupt")?,
        &binary,
    ));
    let operation_id = Uuid::now_v7();
    let mut run = adapter
        .resume(ResumeRequest {
            operation_id,
            conversation_id: "thr_existing".to_owned(),
            prompt: "Continue".to_owned(),
            model: None,
            mode: ExecutionMode::Plan,
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

#[test]
fn binary_resolution_honours_the_explicit_path() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let binary = directory.path().join("codex-custom");
    std::fs::write(&binary, "fixture")?;
    let profile = CodexProfile::new(ProviderAdapterId::new("codex-custom")?, &binary);
    assert_eq!(resolve_binary(&profile).as_deref(), Some(binary.as_path()));
    Ok(())
}
