use std::{collections::HashSet, net::TcpListener as StdListener, time::Instant};

use homebot_protocol::{BotColor, BotPermissionProfile, BotShape};
use tempfile::TempDir;
use tokio::sync::oneshot;

use super::*;

fn unused_endpoint() -> Result<String, Box<dyn std::error::Error>> {
    let listener = StdListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    drop(listener);
    Ok(format!("http://{address}"))
}

fn draft(name: &str) -> BotEditorDraft {
    BotEditorDraft {
        bot_id: None,
        name: name.to_owned(),
        title: "Local helper".to_owned(),
        description: "Created through the authenticated desktop API".to_owned(),
        shape: BotShape::RoundedSquare,
        color: BotColor::Violet,
        provider_profile_id: None,
        permission_profile: BotPermissionProfile::AskBeforeChanges,
    }
}

fn receive_until(
    transport: &DesktopTransport,
    timeout: Duration,
    predicate: impl Fn(&DesktopEvent) -> bool,
) -> Result<Vec<DesktopEvent>, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let mut received = Vec::new();
    while started.elapsed() < timeout {
        let remaining = timeout.saturating_sub(started.elapsed());
        let event = transport.events.recv_timeout(remaining).map_err(|error| {
            format!("desktop event wait failed ({error:?}); received {received:?}")
        })?;
        let found = predicate(&event);
        received.push(event);
        if found {
            return Ok(received);
        }
    }
    Err("timed out waiting for desktop transport event".into())
}

#[test]
fn clean_local_launch_supervises_server_and_persists_real_api_state()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let config = RuntimeConfig {
        endpoint: unused_endpoint()?,
        device_token: "local-desktop-token".to_owned(),
        local_database: Some(directory.path().join("homebot.db")),
        reconnect_delay: Duration::from_millis(20),
    };
    let transport = DesktopTransport::start(config.clone());
    let initial = receive_until(&transport, Duration::from_secs(10), |event| {
        matches!(event, DesktopEvent::Snapshot { .. })
    })?;
    assert!(
        initial
            .iter()
            .any(|event| matches!(event, DesktopEvent::Connected))
    );

    transport.send(DesktopCommand::Bot(BotClientCommand::Create(draft("Nova"))))?;
    let created = receive_until(&transport, Duration::from_secs(10), |event| {
        matches!(event, DesktopEvent::BotMutation(_))
    })?;
    let bot = created
        .iter()
        .find_map(|event| match event {
            DesktopEvent::BotMutation(response) => Some(response.bot.clone()),
            _ => None,
        })
        .ok_or("missing Bot mutation")?;

    transport.send(DesktopCommand::Timeline {
        bot_id: bot.id,
        chat_id: None,
        command: TimelineCommand::Send(ComposerDraft {
            content: "Hello from desktop".to_owned(),
            ..ComposerDraft::default()
        }),
    })?;
    let message_events = receive_until(&transport, Duration::from_secs(10), |event| {
        matches!(
            event,
            DesktopEvent::Server(ServerEvent {
                body: ServerEventBody::MessageChanged { .. },
                ..
            })
        )
    })?;
    assert!(message_events.iter().any(|event| matches!(
        event,
        DesktopEvent::Server(ServerEvent {
            body: ServerEventBody::ChatChanged { .. },
            ..
        })
    )));

    transport.send(DesktopCommand::UploadAttachment {
        filename: "note.txt".to_owned(),
        media_type: "text/plain".to_owned(),
        bytes: b"durable attachment".to_vec(),
    })?;
    let _ = receive_until(&transport, Duration::from_secs(10), |event| {
        matches!(event, DesktopEvent::AttachmentUploaded(_))
    })?;
    drop(transport);

    let restarted = DesktopTransport::start(config);
    let snapshot = receive_until(&restarted, Duration::from_secs(10), |event| {
        matches!(event, DesktopEvent::Snapshot { .. })
    })?
    .into_iter()
    .find_map(|event| match event {
        DesktopEvent::Snapshot { snapshot, .. } => Some(snapshot),
        _ => None,
    })
    .ok_or("missing restart snapshot")?;
    assert_eq!(snapshot.bots.len(), 1);
    assert_eq!(snapshot.bots[0].name, "Nova");
    assert_eq!(snapshot.chats.len(), 1);
    Ok(())
}

async fn spawn_server(
    endpoint: &str,
    database: &std::path::Path,
    token: &str,
) -> Result<(oneshot::Sender<()>, tokio::task::JoinHandle<()>), Box<dyn std::error::Error>> {
    let address = endpoint_socket(endpoint)?;
    let listener = TcpListener::bind(address).await?;
    let storage = homebot_storage::Storage::open(database).await?;
    let state = homebot_server::AppState::new(storage, token);
    let (shutdown, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = homebot_server::serve(listener, state, shutdown_rx).await;
    });
    Ok((shutdown, task))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnect_resumes_without_duplicate_events_after_server_restart()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let endpoint = unused_endpoint()?;
    let database = directory.path().join("homebot.db");
    let token = "remote-desktop-token";
    let (shutdown, task) = spawn_server(&endpoint, &database, token).await?;
    let transport = DesktopTransport::start(RuntimeConfig {
        endpoint: endpoint.clone(),
        device_token: token.to_owned(),
        local_database: None,
        reconnect_delay: Duration::from_millis(20),
    });
    let _ = receive_until(&transport, Duration::from_secs(10), |event| {
        matches!(event, DesktopEvent::Snapshot { .. })
    })
    .map_err(|error| format!("initial snapshot: {error}"))?;
    transport.send(DesktopCommand::Bot(BotClientCommand::Create(draft(
        "Patch",
    ))))?;
    let before = receive_until(&transport, Duration::from_secs(10), |event| {
        matches!(
            event,
            DesktopEvent::Server(ServerEvent {
                body: ServerEventBody::BotChanged { .. },
                ..
            })
        )
    })
    .map_err(|error| format!("created Bot event: {error}"))?;
    let bot = before
        .iter()
        .find_map(|event| match event {
            DesktopEvent::Server(ServerEvent {
                body: ServerEventBody::BotChanged { bot },
                ..
            }) => Some(bot.clone()),
            _ => None,
        })
        .ok_or("missing Bot event")?;
    let mut event_ids: HashSet<Uuid> = before
        .iter()
        .filter_map(|event| match event {
            DesktopEvent::Server(event) => Some(event.event_id),
            _ => None,
        })
        .collect();

    let _ = shutdown.send(());
    task.await?;
    let _ = receive_until(&transport, Duration::from_secs(10), |event| {
        matches!(event, DesktopEvent::Disconnected(_))
    })
    .map_err(|error| format!("disconnect: {error}"))?;
    let (shutdown_again, task_again) = spawn_server(&endpoint, &database, token).await?;
    let resumed = receive_until(&transport, Duration::from_secs(10), |event| {
        matches!(event, DesktopEvent::Connected)
    })
    .map_err(|error| format!("resume: {error}"))?;
    for event in resumed {
        if let DesktopEvent::Server(event) = event {
            assert!(
                event_ids.insert(event.event_id),
                "replay duplicated an applied event"
            );
        }
    }
    transport.send(DesktopCommand::Bot(BotClientCommand::Archive(bot.id)))?;
    let after = receive_until(
            &transport,
            Duration::from_secs(10),
            |event| matches!(event, DesktopEvent::Server(ServerEvent { body: ServerEventBody::BotChanged { bot }, .. }) if bot.archived),
        )
        .map_err(|error| format!("archive after resume: {error}"))?;
    for event in after {
        if let DesktopEvent::Server(event) = event {
            assert!(
                event_ids.insert(event.event_id),
                "event was delivered more than once"
            );
        }
    }
    drop(transport);
    let _ = shutdown_again.send(());
    task_again.await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authentication_version_and_unavailable_failures_are_distinct()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = TempDir::new()?;
    let endpoint = unused_endpoint()?;
    let redacted = RuntimeConfig {
        endpoint: endpoint.clone(),
        device_token: "must-not-appear".to_owned(),
        local_database: None,
        reconnect_delay: Duration::from_secs(1),
    };
    assert!(!format!("{redacted:?}").contains("must-not-appear"));
    let (shutdown, task) =
        spawn_server(&endpoint, &directory.path().join("auth.db"), "correct").await?;
    let wrong = DesktopTransport::start(RuntimeConfig {
        endpoint: endpoint.clone(),
        device_token: "wrong".to_owned(),
        local_database: None,
        reconnect_delay: Duration::from_secs(1),
    });
    let auth = receive_until(&wrong, Duration::from_secs(10), |event| {
        matches!(
            event,
            DesktopEvent::Disconnected(TransportFailure::AuthenticationFailed)
        )
    })?;
    assert!(auth.iter().any(|event| matches!(
        event,
        DesktopEvent::Disconnected(TransportFailure::AuthenticationFailed)
    )));
    drop(wrong);
    let _ = shutdown.send(());
    task.await?;

    let unavailable = DesktopTransport::start(RuntimeConfig {
        endpoint: unused_endpoint()?,
        device_token: "unused".to_owned(),
        local_database: None,
        reconnect_delay: Duration::from_secs(1),
    });
    let _ = receive_until(&unavailable, Duration::from_secs(10), |event| {
        matches!(
            event,
            DesktopEvent::Disconnected(TransportFailure::ServerUnavailable)
        )
    })?;
    drop(unavailable);

    let mismatch_endpoint = unused_endpoint()?;
    let listener = TcpListener::bind(endpoint_socket(&mismatch_endpoint)?).await?;
    let app = axum::Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route(
            "/api/v1/version",
            axum::routing::get(|| async { StatusCode::UPGRADE_REQUIRED }),
        );
    let mismatch_task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let mismatch = DesktopTransport::start(RuntimeConfig {
        endpoint: mismatch_endpoint,
        device_token: "unused".to_owned(),
        local_database: None,
        reconnect_delay: Duration::from_secs(1),
    });
    let _ = receive_until(&mismatch, Duration::from_secs(10), |event| {
        matches!(
            event,
            DesktopEvent::Disconnected(TransportFailure::VersionMismatch)
        )
    })?;
    drop(mismatch);
    mismatch_task.abort();
    Ok(())
}
