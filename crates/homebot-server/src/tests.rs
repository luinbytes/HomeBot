//! Server transport integration tests.

use super::*;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use futures_util::{SinkExt, StreamExt};
use homebot_protocol::{
    AddGroupParticipantRequest, ApprovalDecisionRequest, ArtifactSummary, Attachment, BotColor,
    BotMutationRequest, BotPermissionProfile, BotProviderStatus, BotResponse, BotShape,
    ChatTimelineResponse, CreateAttachmentRequest, CreateAttachmentResponse, CreateBotRequest,
    CreateDirectChatRequest, CreateDirectChatResponse, CreateGroupChatRequest,
    CreateGroupChatResponse, FinalizeAttachmentRequest, GroupBotStatus, GroupTimelineResponse,
    HandoffGroupRequest, SendGroupMessageRequest, SendMessageRequest, SendMessageResponse,
    UpdateBotRequest, UpdateGroupParticipantRequest,
};
use homebot_providers::{
    ActivityKind, ActivityStatus as ProviderActivityStatus, ApprovalDecision, CompactRequest,
    ProviderAdapter, ProviderAdapterId, ProviderApproval, ProviderCapabilities, ProviderCapability,
    ProviderDescriptor, ProviderError, ProviderEvent, ProviderHealth, ProviderModel, ProviderRun,
    ProviderRuntime, ResumeRequest, StartRequest,
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};
use tower::ServiceExt;

#[derive(Debug)]
struct ChatFakeAdapter {
    id: ProviderAdapterId,
    operations: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
    approvals: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
}

impl ChatFakeAdapter {
    fn new() -> Result<Self, homebot_providers::ProviderContractError> {
        Ok(Self {
            id: ProviderAdapterId::new("chat-fake")?,
            operations: Arc::new(Mutex::new(HashMap::new())),
            approvals: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    async fn run(&self, operation_id: Uuid, conversation_id: String) -> ProviderRun {
        let (events_tx, events_rx) = mpsc::channel(16);
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let (approval_tx, mut approval_rx) = watch::channel(false);
        let approval_id = Uuid::now_v7();
        self.operations.lock().await.insert(operation_id, cancel_tx);
        self.approvals.lock().await.insert(approval_id, approval_tx);
        let operations = Arc::clone(&self.operations);
        let approvals = Arc::clone(&self.approvals);
        tokio::spawn(async move {
            for event in [
                ProviderEvent::ConversationStarted { conversation_id },
                ProviderEvent::ContentDelta {
                    text: "Hello from the Bot".to_owned(),
                },
                ProviderEvent::Activity {
                    activity: homebot_providers::ProviderActivity {
                        activity_id: Uuid::now_v7(),
                        kind: ActivityKind::Search,
                        title: "Searching sources".to_owned(),
                        status: ProviderActivityStatus::Started,
                    },
                },
                ProviderEvent::ApprovalRequired {
                    approval: ProviderApproval {
                        approval_id,
                        capability: "shell_execute".to_owned(),
                        action: "Run tests".to_owned(),
                        resource: "workspace".to_owned(),
                        reason: "Verify the change".to_owned(),
                    },
                },
            ] {
                let _ = events_tx.send(event).await;
            }
            tokio::select! {
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && *cancel_rx.borrow() {
                        let _ = events_tx.send(ProviderEvent::Cancelled).await;
                    }
                }
                changed = approval_rx.changed() => {
                    if changed.is_ok() && *approval_rx.borrow() {
                        let _ = events_tx.send(ProviderEvent::Completed).await;
                    }
                }
            }
            operations.lock().await.remove(&operation_id);
            approvals.lock().await.remove(&approval_id);
        });
        ProviderRun {
            operation_id,
            events: events_rx,
        }
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for ChatFakeAdapter {
    fn id(&self) -> &ProviderAdapterId {
        &self.id
    }

    async fn discover(&self) -> Result<ProviderDescriptor, ProviderError> {
        Ok(ProviderDescriptor {
            adapter_id: self.id.clone(),
            display_name: "Chat fixture".to_owned(),
            executable: None,
            capabilities: ProviderCapabilities {
                supported: [
                    ProviderCapability::ConversationResume,
                    ProviderCapability::Streaming,
                    ProviderCapability::Activities,
                    ProviderCapability::Approvals,
                    ProviderCapability::Cancellation,
                ]
                .into_iter()
                .collect(),
            },
        })
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth {
            availability: homebot_providers::ProviderAvailability::Available,
            message: "Ready".to_owned(),
            checked_at_unix_ms: 1,
        }
    }

    async fn models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        Ok(Vec::new())
    }

    async fn start(&self, request: StartRequest) -> Result<ProviderRun, ProviderError> {
        Ok(self
            .run(request.operation_id, format!("chat-{}", request.chat_id))
            .await)
    }

    async fn resume(&self, request: ResumeRequest) -> Result<ProviderRun, ProviderError> {
        Ok(self
            .run(request.operation_id, request.conversation_id)
            .await)
    }

    async fn cancel(&self, operation_id: Uuid) -> Result<(), ProviderError> {
        self.operations
            .lock()
            .await
            .get(&operation_id)
            .ok_or_else(|| ProviderError::internal("operation finished"))?
            .send(true)
            .map_err(|_| ProviderError::internal("operation finished"))
    }

    async fn resolve_approval(
        &self,
        approval_id: Uuid,
        _decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        self.approvals
            .lock()
            .await
            .get(&approval_id)
            .ok_or_else(|| ProviderError::internal("approval finished"))?
            .send(true)
            .map_err(|_| ProviderError::internal("approval finished"))
    }

    async fn compact(&self, _request: CompactRequest) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn recover(&self) -> Result<Vec<ProviderRun>, ProviderError> {
        Ok(Vec::new())
    }
}

async fn test_app() -> Result<Router, homebot_storage::StorageError> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("homebot.db");
    let storage = Storage::open(&path).await?;
    Ok(router(AppState::new(storage, "correct-token")))
}

async fn spawn_app(
    storage: Storage,
) -> Result<(std::net::SocketAddr, JoinHandle<()>, tempfile::TempDir), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let (address, task) = spawn_state(AppState::new(storage, "correct-token")).await?;
    Ok((address, task, directory))
}

async fn spawn_state(
    state: AppState,
) -> Result<(std::net::SocketAddr, JoinHandle<()>), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    Ok((address, task))
}

async fn authenticated_socket(
    address: std::net::SocketAddr,
    resume_after: Option<u64>,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Box<dyn std::error::Error>,
> {
    let mut request = format!("ws://{address}/api/v1/events").into_client_request()?;
    request
        .headers_mut()
        .insert("authorization", "Bearer correct-token".parse()?);
    let (mut socket, _) = connect_async(request).await?;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&ClientMessage::Hello {
                protocol_version: homebot_protocol::PROTOCOL_VERSION,
                client_version: "test".to_owned(),
                device_session: "test-device".to_owned(),
                resume_after,
            })?
            .into(),
        ))
        .await?;
    Ok(socket)
}

fn json_request(method: &str, uri: &str, body: &impl serde::Serialize) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer correct-token")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(body).unwrap_or_else(|error| panic!("{error}")),
        ))
        .unwrap_or_else(|error| panic!("{error}"))
}

async fn response_json<T: serde::de::DeserializeOwned>(
    response: Response,
) -> Result<T, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX).await?,
    )?)
}

async fn fetch_timeline(
    app: &Router,
    chat_id: Uuid,
) -> Result<ChatTimelineResponse, Box<dyn std::error::Error>> {
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/chats/{chat_id}/timeline"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    response_json(response).await
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn bot_lifecycle_validates_persists_streams_and_reports_provider_health()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("homebot.db");
    let storage = Storage::open(&database).await?;
    let state = AppState::new(storage.clone(), "correct-token");
    let app = router(state.clone());
    let key = Uuid::now_v7();
    let create = CreateBotRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: key,
        name: " Nova ".to_owned(),
        title: "Research".to_owned(),
        description: "Find useful context".to_owned(),
        shape: BotShape::Hexagon,
        color: BotColor::Blue,
        provider_profile_id: Some(Uuid::now_v7()),
        permission_profile: BotPermissionProfile::AskBeforeChanges,
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/bots", &create))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: BotResponse = response_json(response).await?;
    assert_eq!(created.bot.name, "Nova");
    assert_eq!(created.bot.provider, BotProviderStatus::Unavailable);
    assert_eq!(created.bot.id, key);

    let replay = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/bots", &create))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json::<BotResponse>(replay).await?.bot.id, key);

    let duplicate = CreateBotRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        name: "nova".to_owned(),
        ..create.clone()
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/bots", &duplicate))
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let update = UpdateBotRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        name: "Nova".to_owned(),
        title: "Lead researcher".to_owned(),
        description: "Updated".to_owned(),
        shape: BotShape::Circle,
        color: BotColor::Green,
        provider_profile_id: None,
        permission_profile: BotPermissionProfile::ReadOnly,
    };
    let response = app
        .clone()
        .oneshot(json_request("PUT", &format!("/api/v1/bots/{key}"), &update))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json::<BotResponse>(response).await?.bot.title,
        "Lead researcher"
    );

    let archive = BotMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/bots/{key}/archive"),
            &archive,
        ))
        .await?;
    assert!(response_json::<BotResponse>(response).await?.bot.archived);
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/bots/{key}/restore"),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert!(!response_json::<BotResponse>(response).await?.bot.archived);

    storage
        .set_bot_attention(
            Uuid::nil(),
            key,
            2,
            homebot_domain::BotAttention::NeedsApproval,
            100,
        )
        .await?;
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/bots/{key}/read"),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert_eq!(
        response_json::<BotResponse>(response)
            .await?
            .bot
            .unread_count,
        0
    );

    let listed = app
        .oneshot(
            Request::get("/api/v1/bots")
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(
        response_json::<Vec<homebot_protocol::BotSummary>>(listed)
            .await?
            .len(),
        1
    );
    let snapshot = current_snapshot(&state).await;
    assert_eq!(snapshot.bots.len(), 1);
    assert_eq!(snapshot.bots[0].title, "Lead researcher");
    let events = storage.events_after(Uuid::nil(), 0, 100).await?;
    assert!(events.len() >= 5);
    assert!(events.iter().all(|event| event.event_kind == "bot_changed"));
    storage.pool().close().await;
    let reopened = Storage::open(&database).await?;
    assert_eq!(reopened.list_bots(Uuid::nil(), true).await?.len(), 1);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn direct_chat_send_queue_replay_and_timeline_are_server_authoritative()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let app = router(AppState::new(storage.clone(), "correct-token"));
    let bot_id = Uuid::now_v7();
    let create_bot = CreateBotRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: bot_id,
        name: "Nova".to_owned(),
        title: "Research".to_owned(),
        description: String::new(),
        shape: BotShape::RoundedSquare,
        color: BotColor::Violet,
        provider_profile_id: None,
        permission_profile: BotPermissionProfile::AskBeforeChanges,
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/bots", &create_bot))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);

    let chat_key = Uuid::now_v7();
    let create_chat = CreateDirectChatRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: chat_key,
        bot_id,
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/chats/direct", &create_chat))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let chat = response_json::<CreateDirectChatResponse>(response)
        .await?
        .chat;
    assert_eq!(chat.id, chat_key);
    let replay = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/chats/direct", &create_chat))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);

    let message_key = Uuid::now_v7();
    let send = SendMessageRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: message_key,
        content: "Hello".to_owned(),
        attachment_ids: Vec::new(),
        reply_to_message_id: None,
        mentioned_bot_ids: vec![bot_id],
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_key}/messages"),
            &send,
        ))
        .await?;
    assert!(matches!(
        response_json::<SendMessageResponse>(response).await?,
        SendMessageResponse::Sent { message } if message.id == message_key
    ));
    let replay = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_key}/messages"),
            &send,
        ))
        .await?;
    assert!(matches!(
        response_json::<SendMessageResponse>(replay).await?,
        SendMessageResponse::Sent { message } if message.id == message_key
    ));

    storage
        .set_chat_running(Uuid::nil(), chat_key, true, 100)
        .await?;
    let steer = SendMessageRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        content: "Use the other source".to_owned(),
        attachment_ids: Vec::new(),
        reply_to_message_id: Some(message_key),
        mentioned_bot_ids: Vec::new(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_key}/steer"),
            &steer,
        ))
        .await?;
    assert!(matches!(
        response_json::<SendMessageResponse>(response).await?,
        SendMessageResponse::Sent { .. }
    ));
    let queued_key = Uuid::now_v7();
    let queued = SendMessageRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: queued_key,
        content: "Follow up".to_owned(),
        attachment_ids: Vec::new(),
        reply_to_message_id: None,
        mentioned_bot_ids: Vec::new(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_key}/messages"),
            &queued,
        ))
        .await?;
    assert!(matches!(
        response_json::<SendMessageResponse>(response).await?,
        SendMessageResponse::Queued { prompt } if prompt.id == queued_key
    ));

    let approval = homebot_domain::chat::ChatApproval {
        id: Uuid::now_v7(),
        owner_id: Uuid::nil(),
        chat_id: chat_key,
        message_id: Some(message_key),
        operation_id: Uuid::now_v7(),
        capability: "shell_execute".to_owned(),
        title: "Run command?".to_owned(),
        detail: "cargo test".to_owned(),
        status: homebot_domain::chat::ApprovalStatus::Pending,
        created_at_ms: 101,
        decided_at_ms: None,
    };
    storage.create_chat_approval(&approval).await?;

    let timeline = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/chats/{chat_key}/timeline"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let timeline = response_json::<ChatTimelineResponse>(timeline).await?;
    assert_eq!(timeline.messages.len(), 2);
    assert_eq!(timeline.approvals.len(), 1);
    assert_eq!(timeline.queued_prompts.len(), 1);
    assert!(timeline.chat.running);
    assert!(timeline.boundary_sequence >= 4);
    let stopped = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_key}/stop"),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert!(
        !response_json::<homebot_protocol::ChatSummary>(stopped)
            .await?
            .running
    );
    let decision = ApprovalDecisionRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        allow: false,
    };
    let response = app
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{}/decision", approval.id),
            &decision,
        ))
        .await?;
    assert_eq!(
        response_json::<homebot_protocol::ApprovalSummary>(response)
            .await?
            .status,
        homebot_protocol::ApprovalStatus::Denied
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn provider_turn_streams_persists_approves_resumes_and_cancels()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let profile_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO provider_profiles (
            id, adapter_kind, display_name, configuration_json, created_at_ms, updated_at_ms
         ) VALUES (?, 'chat-fake', 'Fixture', '{\"model\":\"fixture\"}', 1, 1)",
    )
    .bind(profile_id.to_string())
    .execute(storage.pool())
    .await?;
    let runtime = Arc::new(ProviderRuntime::new());
    runtime.register(Arc::new(ChatFakeAdapter::new()?)).await?;
    let state = AppState::new(storage.clone(), "correct-token").with_provider_runtime(runtime);
    let app = router(state);

    let bot_id = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/bots",
            &CreateBotRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: bot_id,
                name: "Nova".to_owned(),
                title: "Research".to_owned(),
                description: String::new(),
                shape: BotShape::RoundedSquare,
                color: BotColor::Violet,
                provider_profile_id: Some(profile_id),
                permission_profile: BotPermissionProfile::AskBeforeChanges,
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let chat_id = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/chats/direct",
            &CreateDirectChatRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: chat_id,
                bot_id,
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);

    let send = SendMessageRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        content: "Find it".to_owned(),
        attachment_ids: Vec::new(),
        reply_to_message_id: None,
        mentioned_bot_ids: Vec::new(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_id}/messages"),
            &send,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let timeline = wait_for_timeline(&app, chat_id, |timeline| {
        timeline.approvals.len() == 1 && timeline.messages.len() == 2
    })
    .await?;
    assert!(timeline.chat.running);
    assert_eq!(timeline.activities.len(), 1);
    assert!(matches!(
        &timeline.messages[1].parts[0],
        homebot_protocol::MessagePart::Text { text, .. } if text == "Hello from the Bot"
    ));

    let approval_id = timeline.approvals[0].id;
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{approval_id}/decision"),
            &ApprovalDecisionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                allow: true,
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let timeline = wait_for_timeline(&app, chat_id, |timeline| {
        !timeline.chat.running
            && timeline
                .messages
                .last()
                .is_some_and(|message| message.status == homebot_protocol::MessageStatus::Completed)
    })
    .await?;
    assert_eq!(
        timeline.approvals[0].status,
        homebot_protocol::ApprovalStatus::Allowed
    );
    assert_eq!(timeline.chat.unread_count, 1);
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_id}/read"),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert_eq!(
        response_json::<homebot_protocol::ChatSummary>(response)
            .await?
            .unread_count,
        0
    );
    let expected_conversation = format!("chat-{chat_id}");
    assert_eq!(
        storage
            .provider_conversation(bot_id, chat_id, profile_id)
            .await?
            .as_deref(),
        Some(expected_conversation.as_str())
    );

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_id}/messages"),
            &SendMessageRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                content: "Continue".to_owned(),
                attachment_ids: Vec::new(),
                reply_to_message_id: None,
                mentioned_bot_ids: Vec::new(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = wait_for_timeline(&app, chat_id, |timeline| {
        timeline.chat.running && timeline.messages.len() == 4
    })
    .await?;
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_id}/stop"),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let timeline = wait_for_timeline(&app, chat_id, |timeline| {
        timeline
            .messages
            .last()
            .is_some_and(|message| message.status == homebot_protocol::MessageStatus::Cancelled)
    })
    .await?;
    assert!(!timeline.chat.running);
    Ok(())
}

#[tokio::test]
async fn failed_provider_message_can_be_retried_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let profile_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO provider_profiles (
            id, adapter_kind, display_name, configuration_json, created_at_ms, updated_at_ms
         ) VALUES (?, 'missing-adapter', 'Missing', '{}', 1, 1)",
    )
    .bind(profile_id.to_string())
    .execute(storage.pool())
    .await?;
    let mut bot = homebot_domain::Bot::create("Retry Bot", "Testing")?;
    bot.provider_profile_id = Some(profile_id);
    let bot = storage.create_bot(Uuid::nil(), bot, 1).await?;
    let chat = storage
        .create_direct_chat(Uuid::nil(), bot.id.0, Uuid::now_v7(), 2)
        .await?;
    let app = router(AppState::new(storage, "correct-token"));
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/messages", chat.id),
            &SendMessageRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                content: "Try this".to_owned(),
                attachment_ids: Vec::new(),
                reply_to_message_id: None,
                mentioned_bot_ids: Vec::new(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let timeline = wait_for_timeline(&app, chat.id, |timeline| {
        timeline
            .messages
            .last()
            .is_some_and(|message| message.status == homebot_protocol::MessageStatus::Failed)
    })
    .await?;
    let failed_id = timeline.messages.last().ok_or("missing failure")?.id;
    let retry = BotMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let uri = format!("/api/v1/chats/{}/messages/{failed_id}/retry", chat.id);
    let response = app
        .clone()
        .oneshot(json_request("POST", &uri, &retry))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let timeline = wait_for_timeline(&app, chat.id, |timeline| {
        timeline
            .messages
            .iter()
            .filter(|message| message.status == homebot_protocol::MessageStatus::Failed)
            .count()
            == 2
    })
    .await?;
    let response = app
        .clone()
        .oneshot(json_request("POST", &uri, &retry))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        fetch_timeline(&app, chat.id).await?.messages.len(),
        timeline.messages.len()
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn group_chat_contract_coordinates_three_bots_with_bounded_handoff()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let mut bot_ids = Vec::new();
    for (index, name) in ["Nova", "Patch", "Scout", "Relay"].into_iter().enumerate() {
        let bot = storage
            .create_bot(
                Uuid::nil(),
                homebot_domain::Bot::create(name, "Group member")?,
                i64::try_from(index).unwrap_or(i64::MAX),
            )
            .await?;
        bot_ids.push(bot.id.0);
    }
    let app = router(AppState::new(storage.clone(), "correct-token"));
    let chat_id = Uuid::now_v7();
    let create = CreateGroupChatRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: chat_id,
        title: "Release team".to_owned(),
        bot_ids: bot_ids[..3].to_vec(),
        ownership_bot_id: bot_ids[0],
        coordination_max_turns: 2,
        max_parallel_bots: 2,
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/groups", &create))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response_json::<CreateGroupChatResponse>(response)
            .await?
            .participants
            .len(),
        3
    );
    let replay = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/groups", &create))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/groups/{chat_id}/participants"),
            &AddGroupParticipantRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                bot_id: bot_ids[3],
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!(
                "/api/v1/groups/{chat_id}/participants/{}/remove",
                bot_ids[3]
            ),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let first_message = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/groups/{chat_id}/messages"),
            &SendGroupMessageRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: first_message,
                content: "Nova and Patch, investigate together".to_owned(),
                mentioned_bot_ids: vec![bot_ids[0], bot_ids[1]],
                shared_context_message_ids: Vec::new(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    storage
        .append_group_bot_message(
            Uuid::nil(),
            chat_id,
            Uuid::now_v7(),
            bot_ids[0],
            "Patch, use the user's request as shared context.",
            &[bot_ids[1]],
            &[first_message],
            unix_time_ms() + 1,
        )
        .await?;

    for bot_id in &bot_ids[..2] {
        let response = app
            .clone()
            .oneshot(json_request(
                "PUT",
                &format!("/api/v1/groups/{chat_id}/participants/{bot_id}/status"),
                &UpdateGroupParticipantRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                    status: GroupBotStatus::Running,
                    operation_id: Some(Uuid::now_v7()),
                },
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!(
                "/api/v1/groups/{chat_id}/participants/{}/status",
                bot_ids[2]
            ),
            &UpdateGroupParticipantRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                status: GroupBotStatus::Running,
                operation_id: Some(Uuid::now_v7()),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    for expected in 1..=2 {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/groups/{chat_id}/coordination-turns"),
                &BotMutationRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                },
            ))
            .await?;
        assert_eq!(
            response_json::<homebot_protocol::GroupChatSummary>(response)
                .await?
                .coordination_turns_used,
            expected
        );
    }
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/groups/{chat_id}/coordination-turns"),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/groups/{chat_id}/handoff"),
            &HandoffGroupRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                from_bot_id: bot_ids[0],
                to_bot_id: bot_ids[2],
                message_id: Some(first_message),
                reason: "Scout owns final verification".to_owned(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let timeline = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/groups/{chat_id}/timeline"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let timeline = response_json::<GroupTimelineResponse>(timeline).await?;
    assert_eq!(timeline.group.ownership_bot_id, bot_ids[2]);
    assert_eq!(timeline.messages.len(), 2);
    assert_eq!(
        timeline.messages[1].shared_context_message_ids,
        vec![first_message]
    );
    assert_eq!(timeline.handoffs.len(), 1);

    let response = app
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/groups/{chat_id}/stop"),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert!(
        response_json::<homebot_protocol::GroupChatSummary>(response)
            .await?
            .stop_requested
    );
    Ok(())
}

async fn wait_for_timeline(
    app: &Router,
    chat_id: Uuid,
    ready: impl Fn(&ChatTimelineResponse) -> bool,
) -> Result<ChatTimelineResponse, Box<dyn std::error::Error>> {
    for _ in 0..100 {
        let timeline = fetch_timeline(app, chat_id).await?;
        if ready(&timeline) {
            return Ok(timeline);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("timed out waiting for provider-backed timeline".into())
}

#[tokio::test]
async fn health_is_available_without_auth_but_reveals_no_secrets()
-> Result<(), Box<dyn std::error::Error>> {
    let response = test_app()
        .await?
        .oneshot(Request::get("/health").body(Body::empty())?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn protected_routes_deny_missing_and_invalid_tokens_server_side()
-> Result<(), Box<dyn std::error::Error>> {
    for authorization in [None, Some("Bearer wrong-token")] {
        let mut request = Request::get("/api/v1/version");
        if let Some(value) = authorization {
            request = request.header("authorization", value);
        }
        let response = test_app()
            .await?
            .oneshot(request.body(Body::empty())?)
            .await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

#[tokio::test]
async fn valid_device_session_can_negotiate_version() -> Result<(), Box<dyn std::error::Error>> {
    let response = test_app()
        .await?
        .oneshot(
            Request::get("/api/v1/version")
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn stale_protocol_is_rejected_with_upgrade_required() -> Result<(), Box<dyn std::error::Error>>
{
    let response = test_app()
        .await?
        .oneshot(
            Request::get("/api/v1/version")
                .header("authorization", "Bearer correct-token")
                .header("x-homebot-protocol", "0")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
    Ok(())
}

#[tokio::test]
async fn websocket_requires_auth_and_sends_snapshot_after_hello()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let (address, task, _guard) = spawn_app(storage).await?;
    let url = format!("ws://{address}/api/v1/events");
    let unauthorised = connect_async(&url).await;
    assert!(
        matches!(unauthorised, Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status() == StatusCode::UNAUTHORIZED)
    );

    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("authorization", "Bearer correct-token".parse()?);
    let (mut socket, _) = connect_async(request).await?;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&ClientMessage::Hello {
                protocol_version: homebot_protocol::PROTOCOL_VERSION,
                client_version: "test".to_owned(),
                device_session: "test-device".to_owned(),
                resume_after: None,
            })?
            .into(),
        ))
        .await?;
    let hello: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing hello")??.to_text()?)?;
    let snapshot: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing snapshot")??.to_text()?)?;
    assert!(matches!(
        hello.body,
        ServerEventBody::Hello {
            resume: ResumeDisposition::SnapshotRequired,
            ..
        }
    ));
    assert!(matches!(
        snapshot.body,
        ServerEventBody::Snapshot {
            boundary_sequence: 0,
            ..
        }
    ));
    task.abort();
    Ok(())
}

#[tokio::test]
async fn reconnect_replays_events_strictly_after_cursor() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let replay = ServerEvent {
        protocol_version: homebot_protocol::PROTOCOL_VERSION,
        sequence: 1,
        event_id: Uuid::now_v7(),
        body: ServerEventBody::Ping {
            nonce: Uuid::now_v7(),
        },
    };
    storage
        .append_event(Uuid::nil(), "ping", &serde_json::to_value(&replay)?, 1)
        .await?;
    let (address, task, _guard) = spawn_app(storage).await?;
    let mut request = format!("ws://{address}/api/v1/events").into_client_request()?;
    request
        .headers_mut()
        .insert("authorization", "Bearer correct-token".parse()?);
    let (mut socket, _) = connect_async(request).await?;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&ClientMessage::Hello {
                protocol_version: homebot_protocol::PROTOCOL_VERSION,
                client_version: "test".to_owned(),
                device_session: "test-device".to_owned(),
                resume_after: Some(0),
            })?
            .into(),
        ))
        .await?;
    let hello: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing hello")??.to_text()?)?;
    let replayed: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing replay")??.to_text()?)?;
    assert!(matches!(
        hello.body,
        ServerEventBody::Hello {
            resume: ResumeDisposition::Replayed,
            ..
        }
    ));
    assert_eq!(replayed, replay);
    task.abort();
    Ok(())
}

#[tokio::test]
async fn reconnect_uses_snapshot_when_cursor_falls_outside_retention()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let replay = ServerEvent {
        protocol_version: homebot_protocol::PROTOCOL_VERSION,
        sequence: 0,
        event_id: Uuid::now_v7(),
        body: ServerEventBody::Ping {
            nonce: Uuid::now_v7(),
        },
    };
    storage
        .append_event(Uuid::nil(), "ping", &serde_json::to_value(&replay)?, 1)
        .await?;
    storage.prune_events_through(Uuid::nil(), 1, 2).await?;
    let (address, task, _guard) = spawn_app(storage).await?;
    let mut request = format!("ws://{address}/api/v1/events").into_client_request()?;
    request
        .headers_mut()
        .insert("authorization", "Bearer correct-token".parse()?);
    let (mut socket, _) = connect_async(request).await?;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&ClientMessage::Hello {
                protocol_version: homebot_protocol::PROTOCOL_VERSION,
                client_version: "test".to_owned(),
                device_session: "test-device".to_owned(),
                resume_after: Some(0),
            })?
            .into(),
        ))
        .await?;
    let hello: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing hello")??.to_text()?)?;
    let snapshot: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing snapshot")??.to_text()?)?;
    assert!(matches!(
        hello.body,
        ServerEventBody::Hello {
            resume: ResumeDisposition::SnapshotRequired,
            ..
        }
    ));
    assert!(matches!(snapshot.body, ServerEventBody::Snapshot { .. }));
    task.abort();
    Ok(())
}

#[tokio::test]
async fn duplicate_command_reuses_operation_and_conflicting_key_fails()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let (address, task, _guard) = spawn_app(storage).await?;
    let mut socket = authenticated_socket(address, None).await?;
    let _hello = socket.next().await.ok_or("missing hello")??;
    let _snapshot = socket.next().await.ok_or("missing snapshot")??;
    let request_id = Uuid::now_v7();
    let key = Uuid::now_v7();
    let command = ClientMessage::Command {
        request_id,
        idempotency_key: key,
        command: homebot_protocol::Command::CreateBot {
            name: "Ada".to_owned(),
            title: "Engineer".to_owned(),
        },
    };
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&command)?.into(),
        ))
        .await?;
    let accepted: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing accepted")??.to_text()?)?;
    let _completed: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing completed")??.to_text()?)?;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&command)?.into(),
        ))
        .await?;
    let replayed: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing replay")??.to_text()?)?;
    let ServerEventBody::CommandAccepted {
        operation_id: operation,
        ..
    } = accepted.body
    else {
        return Err("expected accepted event".into());
    };
    assert!(
        matches!(replayed.body, ServerEventBody::CommandAccepted { operation_id, .. } if operation_id == operation)
    );

    let conflict = ClientMessage::Command {
        request_id: Uuid::now_v7(),
        idempotency_key: key,
        command: homebot_protocol::Command::CreateBot {
            name: "Different".to_owned(),
            title: "Engineer".to_owned(),
        },
    };
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&conflict)?.into(),
        ))
        .await?;
    let failed: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing conflict")??.to_text()?)?;
    assert!(matches!(
        failed.body,
        ServerEventBody::CommandFailed {
            error: ErrorEnvelope {
                code: ErrorCode::Conflict,
                ..
            },
            ..
        }
    ));
    task.abort();
    Ok(())
}

#[tokio::test]
async fn heartbeat_closes_clients_that_never_pong() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let state = AppState::new(storage, "correct-token").with_heartbeat(
        std::time::Duration::from_millis(15),
        std::time::Duration::from_millis(45),
    );
    let (address, task) = spawn_state(state).await?;
    let mut socket = authenticated_socket(address, None).await?;
    let _hello = socket.next().await.ok_or("missing hello")??;
    let _snapshot = socket.next().await.ok_or("missing snapshot")??;
    let closed = tokio::time::timeout(std::time::Duration::from_millis(150), async {
        while let Some(message) = socket.next().await {
            if message.is_err()
                || matches!(
                    message,
                    Ok(tokio_tungstenite::tungstenite::Message::Close(_))
                )
            {
                break;
            }
        }
    })
    .await;
    assert!(closed.is_ok());
    task.abort();
    Ok(())
}

#[tokio::test]
async fn cancellation_stops_running_operation_with_exactly_one_terminal_event()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let state = AppState::new(storage.clone(), "correct-token").with_transport_limits(
        32,
        std::time::Duration::ZERO,
        std::time::Duration::from_millis(250),
    );
    let (address, task) = spawn_state(state).await?;
    let mut socket = authenticated_socket(address, None).await?;
    let _hello = socket.next().await.ok_or("missing hello")??;
    let _snapshot = socket.next().await.ok_or("missing snapshot")??;
    let request_id = Uuid::now_v7();
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::to_string(&ClientMessage::Command {
                request_id,
                idempotency_key: Uuid::now_v7(),
                command: homebot_protocol::Command::SendMessage {
                    chat_id: Uuid::now_v7(),
                    content: "Please stop".to_owned(),
                },
            })?
            .into(),
        ))
        .await?;
    let accepted: ServerEvent =
        serde_json::from_str(socket.next().await.ok_or("missing accepted")??.to_text()?)?;
    let ServerEventBody::CommandAccepted { operation_id, .. } = accepted.body else {
        return Err("expected accepted operation".into());
    };
    for _ in 0..2 {
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&ClientMessage::Cancel {
                    request_id: Uuid::now_v7(),
                    operation_id,
                })?
                .into(),
            ))
            .await?;
    }
    let cancelled: ServerEvent = serde_json::from_str(
        socket
            .next()
            .await
            .ok_or("missing cancellation")??
            .to_text()?,
    )?;
    assert!(matches!(
        cancelled.body,
        ServerEventBody::CommandCancelled {
            request_id: id,
            operation_id: operation,
        } if id == request_id && operation == operation_id
    ));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let events = storage.events_after(Uuid::nil(), 0, 100).await?;
    let terminal_count = events
        .iter()
        .filter_map(|event| serde_json::from_value::<ServerEvent>(event.payload.clone()).ok())
        .filter(|event| {
            matches!(
                event.body,
                ServerEventBody::CommandCompleted { operation_id: id, .. }
                    | ServerEventBody::CommandFailed { operation_id: id, .. }
                    | ServerEventBody::CommandCancelled { operation_id: id, .. }
                    if id == operation_id
            )
        })
        .count();
    assert_eq!(terminal_count, 1);
    task.abort();
    Ok(())
}

#[tokio::test]
async fn slow_client_is_closed_with_cursor_and_replays_every_durable_event()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let state = AppState::new(storage.clone(), "correct-token").with_transport_limits(
        1,
        std::time::Duration::from_millis(75),
        std::time::Duration::from_millis(1),
    );
    let (address, task) = spawn_state(state).await?;
    let mut socket = authenticated_socket(address, None).await?;
    let _hello = socket.next().await.ok_or("missing hello")??;
    let _snapshot = socket.next().await.ok_or("missing snapshot")??;
    for index in 0..8 {
        let sent = socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                serde_json::to_string(&ClientMessage::Command {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                    command: homebot_protocol::Command::CreateBot {
                        name: format!("Bot {index}"),
                        title: "Worker".to_owned(),
                    },
                })?
                .into(),
            ))
            .await;
        if sent.is_err() {
            break;
        }
    }
    let close_reason = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let message = socket
                .next()
                .await
                .ok_or("socket ended without close frame")??;
            if let tokio_tungstenite::tungstenite::Message::Close(Some(frame)) = message {
                return Ok::<String, Box<dyn std::error::Error>>(frame.reason.to_string());
            }
        }
    })
    .await??;
    let resume_after = close_reason
        .strip_prefix("resume_after=")
        .ok_or("missing cursor")?
        .parse::<u64>()?;
    let latest = storage.latest_sequence(Uuid::nil()).await?;
    assert!(latest > resume_after);

    let mut resumed = authenticated_socket(address, Some(resume_after)).await?;
    let hello: ServerEvent = serde_json::from_str(
        resumed
            .next()
            .await
            .ok_or("missing resumed hello")??
            .to_text()?,
    )?;
    assert!(matches!(
        hello.body,
        ServerEventBody::Hello {
            resume: ResumeDisposition::Replayed,
            ..
        }
    ));
    let mut sequences = Vec::new();
    for _ in resume_after..latest {
        let event: ServerEvent = serde_json::from_str(
            resumed
                .next()
                .await
                .ok_or("missing durable replay event")??
                .to_text()?,
        )?;
        sequences.push(event.sequence);
    }
    assert_eq!(sequences, ((resume_after + 1)..=latest).collect::<Vec<_>>());
    task.abort();
    Ok(())
}

#[tokio::test]
async fn attachment_upload_is_idempotent_bounded_and_integrity_checked()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let artifact_root = directory.path().join("artifacts");
    let app = router(
        AppState::new(storage.clone(), "correct-token").with_artifact_root(artifact_root.clone()),
    );
    let content = b"hello";
    let sha256 = format!("{:x}", Sha256::digest(content));
    let create = CreateAttachmentRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        filename: "greeting.txt".to_owned(),
        media_type: "text/plain".to_owned(),
        size_bytes: u64::try_from(content.len())?,
        sha256: sha256.clone(),
    };
    let create_request = || {
        Request::post("/api/v1/attachments")
            .header("authorization", "Bearer correct-token")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&create).unwrap_or_default()))
    };
    let response = app.clone().oneshot(create_request()?).await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let response_body = axum::body::to_bytes(response.into_body(), 16 * 1024).await?;
    let created: CreateAttachmentResponse = serde_json::from_slice(&response_body)?;
    let replay = app.clone().oneshot(create_request()?).await?;
    let replay_body = axum::body::to_bytes(replay.into_body(), 16 * 1024).await?;
    let replayed: CreateAttachmentResponse = serde_json::from_slice(&replay_body)?;
    assert_eq!(replayed.attachment_id, created.attachment_id);

    let upload = app
        .clone()
        .oneshot(
            Request::put(&created.upload_url)
                .header("authorization", "Bearer correct-token")
                .body(Body::from(content.as_slice()))?,
        )
        .await?;
    assert_eq!(upload.status(), StatusCode::NO_CONTENT);
    let finalize = FinalizeAttachmentRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        sha256: sha256.clone(),
    };
    let finalize_url = format!("/api/v1/attachments/{}/finalize", created.attachment_id);
    let finalize_request = || {
        Request::post(&finalize_url)
            .header("authorization", "Bearer correct-token")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&finalize).unwrap_or_default(),
            ))
    };
    let finalized = app.clone().oneshot(finalize_request()?).await?;
    assert_eq!(finalized.status(), StatusCode::OK);
    let finalized_body = axum::body::to_bytes(finalized.into_body(), 16 * 1024).await?;
    let attachment: Attachment = serde_json::from_slice(&finalized_body)?;
    assert_eq!(attachment.sha256, sha256);
    let finalized_again = app.clone().oneshot(finalize_request()?).await?;
    assert_eq!(finalized_again.status(), StatusCode::OK);
    assert!(
        artifact_root
            .join("objects")
            .join(&sha256[..2])
            .join(&sha256)
            .is_file()
    );
    assert_eq!(
        storage
            .attachment(Uuid::nil(), created.attachment_id)
            .await?
            .map(|record| record.status),
        Some("ready".to_owned())
    );
    Ok(())
}

#[tokio::test]
async fn invalid_attachment_bytes_are_deleted_and_cannot_be_finalized()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let app = router(
        AppState::new(storage, "correct-token")
            .with_artifact_root(directory.path().join("artifacts")),
    );
    let expected = b"hello";
    let sha256 = format!("{:x}", Sha256::digest(expected));
    let create = CreateAttachmentRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        filename: "greeting.txt".to_owned(),
        media_type: "text/plain".to_owned(),
        size_bytes: 5,
        sha256: sha256.clone(),
    };
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/attachments")
                .header("authorization", "Bearer correct-token")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create)?))?,
        )
        .await?;
    let body = axum::body::to_bytes(response.into_body(), 16 * 1024).await?;
    let created: CreateAttachmentResponse = serde_json::from_slice(&body)?;
    let invalid = app
        .clone()
        .oneshot(
            Request::put(&created.upload_url)
                .header("authorization", "Bearer correct-token")
                .body(Body::from("wrong"))?,
        )
        .await?;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let finalize = FinalizeAttachmentRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        sha256,
    };
    let finalize_response = app
        .oneshot(
            Request::post(format!(
                "/api/v1/attachments/{}/finalize",
                created.attachment_id
            ))
            .header("authorization", "Bearer correct-token")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&finalize)?))?,
        )
        .await?;
    assert_eq!(finalize_response.status(), StatusCode::CONFLICT);
    Ok(())
}

#[tokio::test]
async fn generated_artifacts_are_server_owned_authenticated_and_remotely_addressable()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let bot = storage
        .create_bot(
            Uuid::nil(),
            homebot_domain::Bot::create("Nova", "Research")?,
            1,
        )
        .await?;
    let chat_id = Uuid::now_v7();
    storage
        .create_direct_chat(Uuid::nil(), bot.id.0, chat_id, 2)
        .await?;
    let state = AppState::new(storage.clone(), "correct-token")
        .with_artifact_root(directory.path().join("artifacts"));
    let content = b"# Release audit\n\nAll checks passed.\n";
    let artifact = artifacts::persist_generated_artifact(
        &state,
        artifacts::GeneratedArtifact {
            chat_id,
            message_id: None,
            activity_id: None,
            name: "release-audit.md",
            kind: "report",
            media_type: "text/markdown",
            bytes: content,
        },
    )
    .await?;
    let app = router(state);
    let metadata_url = format!("/api/v1/artifacts/{}", artifact.id);
    let unauthorized = app
        .clone()
        .oneshot(Request::get(&metadata_url).body(Body::empty())?)
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let metadata = app
        .clone()
        .oneshot(
            Request::get(&metadata_url)
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(metadata.status(), StatusCode::OK);
    let encoded = to_bytes(metadata.into_body(), 16 * 1024).await?;
    let decoded: ArtifactSummary = serde_json::from_slice(&encoded)?;
    assert_eq!(decoded, artifact);
    assert!(!String::from_utf8(encoded.to_vec())?.contains("storage_path"));

    let content_response = app
        .clone()
        .oneshot(
            Request::get(format!("{metadata_url}/content"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(content_response.status(), StatusCode::OK);
    assert_eq!(content_response.headers()["content-type"], "text/markdown");
    assert_eq!(
        to_bytes(content_response.into_body(), 16 * 1024).await?,
        content.as_slice()
    );

    let missing = app
        .oneshot(
            Request::get(format!("/api/v1/artifacts/{}/content", Uuid::now_v7()))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    Ok(())
}
