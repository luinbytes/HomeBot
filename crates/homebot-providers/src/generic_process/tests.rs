use super::*;

#[cfg(unix)]
#[tokio::test]
async fn fixture_process_receives_request_and_emits_normalized_events()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir()?;
    let binary = directory.path().join("community-provider");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
IFS= read -r request
case "$request" in
  *'"kind":"start"'*) ;;
  *) exit 2 ;;
esac
printf '%s\n' '{"kind":"conversation_started","conversation_id":"community_1"}'
printf '%s\n' '{"kind":"content_delta","text":"Community response"}'
printf '%s\n' '{"kind":"completed"}'
"#,
    )?;
    let mut permissions = std::fs::metadata(&binary)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&binary, permissions)?;
    let adapter = GenericProcessAdapter::new(GenericProcessProfile::new(
        ProviderAdapterId::new("community-fixture")?,
        "Community Fixture",
        &binary,
    ));
    let mut run = adapter
        .start(StartRequest {
            operation_id: Uuid::now_v7(),
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            prompt: "Hello".to_owned(),
            model: None,
            working_directory: None,
            mode: crate::ExecutionMode::Normal,
            attachments: Vec::new(),
            tools: Vec::new(),
        })
        .await?;
    assert!(matches!(
        run.events.recv().await,
        Some(ProviderEvent::ConversationStarted { .. })
    ));
    assert_eq!(
        run.events.recv().await,
        Some(ProviderEvent::ContentDelta {
            text: "Community response".to_owned()
        })
    );
    assert_eq!(run.events.recv().await, Some(ProviderEvent::Completed));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn cancellation_closes_input_and_stops_the_process() -> Result<(), Box<dyn std::error::Error>>
{
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir()?;
    let binary = directory.path().join("community-provider");
    std::fs::write(
        &binary,
        r#"#!/bin/sh
IFS= read -r request
printf '%s\n' '{"kind":"conversation_started","conversation_id":"community_waiting"}'
IFS= read -r continue
"#,
    )?;
    let mut permissions = std::fs::metadata(&binary)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&binary, permissions)?;
    let adapter = GenericProcessAdapter::new(GenericProcessProfile::new(
        ProviderAdapterId::new("community-cancel")?,
        "Community Fixture",
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
            mode: crate::ExecutionMode::Normal,
            attachments: Vec::new(),
            tools: Vec::new(),
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
fn debug_output_excludes_arguments_and_environment_values() -> Result<(), Box<dyn std::error::Error>>
{
    let profile = GenericProcessProfile::new(
        ProviderAdapterId::new("community")?,
        "Community",
        "/provider",
    )
    .argument("--token=secret")
    .environment("TOKEN", "secret");
    let debug = format!("{:?}", GenericProcessAdapter::new(profile));
    assert!(!debug.contains("--token=secret"));
    assert!(!debug.contains("TOKEN\": \"secret"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn simple_command_resolves_only_from_the_selected_search_path()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir()?;
    let binary = directory.path().join("homebot-community-provider");
    std::fs::write(
        &binary,
        "#!/bin/sh\nIFS= read -r request\nprintf '%s\\n' '{\"kind\":\"completed\"}'\n",
    )?;
    let mut permissions = std::fs::metadata(&binary)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&binary, permissions)?;
    let path = std::env::join_paths([directory.path()])?;
    let profile = GenericProcessProfile::new(
        ProviderAdapterId::new("community-path")?,
        "Community PATH fixture",
        "homebot-community-provider",
    )
    .environment("PATH", path);
    let adapter = GenericProcessAdapter::new(profile);

    let expected = binary.to_string_lossy().into_owned();
    assert_eq!(
        adapter.discover().await?.executable.as_deref(),
        Some(expected.as_str())
    );
    let mut run = adapter
        .start(StartRequest {
            operation_id: Uuid::now_v7(),
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            prompt: "Hello".to_owned(),
            model: None,
            working_directory: None,
            mode: crate::ExecutionMode::Normal,
            attachments: Vec::new(),
            tools: Vec::new(),
        })
        .await?;
    assert_eq!(run.events.recv().await, Some(ProviderEvent::Completed));
    Ok(())
}
