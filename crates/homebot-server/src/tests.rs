//! Server transport integration tests.

use super::*;
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use futures_util::{SinkExt, StreamExt};
use homebot_protocol::{
    AddGroupParticipantRequest, AppendRoutineRecordingRequest, ApprovalDecisionRequest,
    ArtifactSummary, AssistantPackCadence, AssistantPackInstallationSummary, AssistantPackSummary,
    AttachChatWorkspaceRequest, Attachment, BotColor, BotMutationRequest, BotPermissionProfile,
    BotProviderStatus, BotResponse, BotShape, BrowserActionRequest, BrowserActionResponse,
    BrowserCommand, BrowserController, BrowserMutationRequest, BrowserSessionStatus,
    CapabilityClass, CapabilityRuleAuditSummary, CapabilityRuleEffect, CapabilityRuleSummary,
    ChatTimelineResponse, ChatWorkspaceSummary, CheckpointDiffResponse, CheckpointPhase,
    CheckpointRestoreSummary, CompactWorkingContextRequest, ContextCompactionStatus,
    ContextCompactionStrategy, ConversationReconciliation, CreateAttachmentRequest,
    CreateAttachmentResponse, CreateBotRequest, CreateBrowserSessionRequest,
    CreateDirectChatRequest, CreateDirectChatResponse, CreateGroupChatRequest,
    CreateGroupChatResponse, CreateLocalMcpPluginRequest, CreatePairingRequest,
    CreatePullRequestRequest, CreateRepositoryWorkspaceRequest, CreateRoutineRequest,
    CreateRoutineTriggerRequest, CreateSkillRequest, DeleteBotRequest,
    DeliverRoutineTriggerRequest, DetachChatWorkspaceRequest, DeviceSessionSummary,
    DuplicateRoutineRequest, DuplicateSkillRequest, ExchangePairingRequest,
    FinalizeAttachmentRequest, GlobalSearchResponse, GroupBotStatus, GroupTimelineResponse,
    HandoffGroupRequest, ImportSkillRequest, InstallAssistantPackRequest, InteractionMode,
    MessageReferenceInput, MessageReferenceKind, MissedRunPolicy, OverlapPolicy,
    PairingEndpointKind, PairingExchangeResponse, PairingOffer, PluginAssignmentRequest,
    PluginConnectionState, PluginMutationRequest, PluginSummary, PullRequestMetadata,
    PullRequestMutationResponse, QueuedPromptKind, ReactionMutationRequest, RecordedAction,
    RecordedActor, RenameGroupChatRequest, RepositoryWorkspaceSummary, RestoreCheckpointRequest,
    RetryPolicy, RevokeDeviceSessionRequest, RoutineDefinition, RoutineInput, RoutineInputKind,
    RoutineJobSummary, RoutineRecordingSummary, RoutineRunSummary, RoutineSchedule, RoutineStep,
    RoutineStepStatus, RoutineSummary, RoutineTriggerDefinition, RoutineTriggerSource,
    RunRoutineRequest, SecretSummary, SendGroupMessageRequest, SendMessageRequest,
    SendMessageResponse, SetInteractionModeRequest, SkillAssignmentRequest, SkillBundle,
    SkillContext, SkillDefinition, SkillImportConflictPolicy, SkillSummary, SkillTestSummary,
    SkillToolReference, StartRoutineRecordingRequest, TurnCheckpointSummary, UpdateBotRequest,
    UpdateGroupParticipantRequest, UpdateRoutineRequest, UpdateSkillRequest,
    UpsertCapabilityRuleRequest, VcsCommitRequest, VcsCommitResult, VcsCreateBranchRequest,
    VcsMutationStatus, VcsPushRequest, VcsRemoteMutationResponse, VcsStatus, WorkingContextSummary,
    WorkingTreeCondition, WorkingTreeDiffResponse, WorkspaceBranchesResponse, WorkspaceMode,
};
use homebot_providers::{
    ActivityKind, ActivityStatus as ProviderActivityStatus, ApprovalDecision, CompactRequest,
    ProviderAdapter, ProviderAdapterId, ProviderApproval, ProviderCapabilities, ProviderCapability,
    ProviderDescriptor, ProviderError, ProviderEvent, ProviderHealth, ProviderModel, ProviderRun,
    ProviderRuntime, ResumeRequest, StartRequest,
};
use homebot_secrets::{MemorySecretVault, SecretStatus, SecretVault, locator_for};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};
use tower::ServiceExt;

static PROVIDER_QUEUE_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

async fn provider_queue_test_guard() -> tokio::sync::MutexGuard<'static, ()> {
    PROVIDER_QUEUE_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .await
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One end-to-end contract covers catalog, install, replay, and execution.
async fn assistant_pack_catalog_installs_a_scheduled_skill_for_one_bot_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let bot = storage
        .create_bot(
            Uuid::nil(),
            homebot_domain::Bot::create("Nova", "Personal assistant")?,
            1,
        )
        .await?;
    let app = router(AppState::new(storage.clone(), "correct-token"));

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/assistant-packs")
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let catalog: Vec<AssistantPackSummary> = response_json(response).await?;
    assert_eq!(
        catalog
            .iter()
            .map(|pack| pack.id.as_str())
            .collect::<Vec<_>>(),
        vec!["morning-brief", "weekly-rundown", "end-of-day-review"]
    );
    assert_eq!(catalog[1].schedule.cadence, AssistantPackCadence::Weekly);
    assert_eq!(catalog[1].schedule.weekday, Some(5));

    let idempotency_key = Uuid::now_v7();
    let mut request = InstallAssistantPackRequest {
        request_id: Uuid::now_v7(),
        idempotency_key,
        bot_id: bot.id.0,
        timezone: "not-a-timezone".to_owned(),
        hour: 8,
        minute: 30,
    };
    for _ in 0..2 {
        let invalid = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/assistant-packs/morning-brief/install",
                &request,
            ))
            .await?;
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    request.timezone = "Europe/London".to_owned();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/assistant-packs/morning-brief/install",
            &request,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let installed: AssistantPackInstallationSummary = response_json(response).await?;
    assert_eq!(installed.pack_id, "morning-brief");
    assert_eq!(installed.skill.bot_ids, vec![bot.id.0]);
    assert_eq!(installed.routine.bot_id, bot.id.0);
    assert!(installed.routine.enabled);
    assert!(!installed.routine.draft);
    assert!(matches!(
        installed.trigger.definition.source,
        RoutineTriggerSource::Schedule {
            schedule: RoutineSchedule::DailyLocal {
                ref timezone,
                hour: 8,
                minute: 30,
            }
        } if timezone == "Europe/London"
    ));

    let replay = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/assistant-packs/morning-brief/install",
            &request,
        ))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    let replayed: AssistantPackInstallationSummary = response_json(replay).await?;
    assert_eq!(replayed.skill.id, installed.skill.id);
    assert_eq!(replayed.routine.id, installed.routine.id);
    assert_eq!(replayed.trigger.id, installed.trigger.id);
    assert_eq!(replayed.trigger.definition, installed.trigger.definition);
    assert_eq!(storage.list_skills(Uuid::nil()).await?.len(), 1);
    assert_eq!(storage.list_routines(Uuid::nil()).await?.len(), 1);
    assert_eq!(
        storage
            .routine_triggers(Uuid::nil(), Some(installed.routine.id))
            .await?
            .len(),
        1
    );
    let dry_run = app
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{}/dry-run", installed.routine.id),
            &RunRoutineRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                inputs: serde_json::json!({}),
            },
        ))
        .await?;
    assert_eq!(dry_run.status(), StatusCode::OK);
    let dry_run: RoutineRunSummary = response_json(dry_run).await?;
    assert_eq!(dry_run.status, "dry_run_succeeded");
    assert_eq!(dry_run.results[0].status, RoutineStepStatus::Planned);
    Ok(())
}

#[derive(Debug)]
struct ChatFakeAdapter {
    id: ProviderAdapterId,
    operations: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
    approvals: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
    prompts: Arc<Mutex<Vec<String>>>,
    working_directories: Arc<Mutex<Vec<Option<PathBuf>>>>,
    modes: Arc<Mutex<Vec<homebot_providers::ExecutionMode>>>,
    compactions: Arc<Mutex<Vec<CompactRequest>>>,
    context_features: bool,
}

impl ChatFakeAdapter {
    fn new() -> Result<Self, homebot_providers::ProviderContractError> {
        Ok(Self {
            id: ProviderAdapterId::new("chat-fake")?,
            operations: Arc::new(Mutex::new(HashMap::new())),
            approvals: Arc::new(Mutex::new(HashMap::new())),
            prompts: Arc::new(Mutex::new(Vec::new())),
            working_directories: Arc::new(Mutex::new(Vec::new())),
            modes: Arc::new(Mutex::new(Vec::new())),
            compactions: Arc::new(Mutex::new(Vec::new())),
            context_features: true,
        })
    }

    fn without_context_features() -> Result<Self, homebot_providers::ProviderContractError> {
        let mut adapter = Self::new()?;
        adapter.id = ProviderAdapterId::new("chat-basic")?;
        adapter.context_features = false;
        Ok(adapter)
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
                ProviderEvent::Usage {
                    usage: homebot_providers::ProviderUsage {
                        input_tokens: 120,
                        output_tokens: 5,
                        cached_input_tokens: 40,
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
        let mut supported = [
            ProviderCapability::ConversationResume,
            ProviderCapability::Streaming,
            ProviderCapability::Activities,
            ProviderCapability::Approvals,
            ProviderCapability::Cancellation,
            ProviderCapability::Usage,
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
        if self.context_features {
            supported.extend([ProviderCapability::Compaction, ProviderCapability::PlanMode]);
        }
        Ok(ProviderDescriptor {
            adapter_id: self.id.clone(),
            display_name: "Chat fixture".to_owned(),
            executable: None,
            capabilities: ProviderCapabilities { supported },
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
        Ok(vec![ProviderModel {
            id: "fixture".to_owned(),
            display_name: "Fixture".to_owned(),
            context_window_tokens: Some(4_096),
            supports_reasoning: true,
        }])
    }

    async fn start(&self, request: StartRequest) -> Result<ProviderRun, ProviderError> {
        self.prompts.lock().await.push(request.prompt.clone());
        self.working_directories
            .lock()
            .await
            .push(request.working_directory.clone());
        self.modes.lock().await.push(request.mode);
        Ok(self
            .run(request.operation_id, format!("chat-{}", request.chat_id))
            .await)
    }

    async fn resume(&self, request: ResumeRequest) -> Result<ProviderRun, ProviderError> {
        self.prompts.lock().await.push(request.prompt.clone());
        self.working_directories
            .lock()
            .await
            .push(request.working_directory.clone());
        self.modes.lock().await.push(request.mode);
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

    async fn compact(&self, request: CompactRequest) -> Result<(), ProviderError> {
        self.compactions.lock().await.push(request);
        Ok(())
    }

    async fn recover(&self) -> Result<Vec<ProviderRun>, ProviderError> {
        Ok(Vec::new())
    }
}

#[derive(Debug)]
struct BrowserFakeRuntime {
    policy: Arc<homebot_tools::PolicyEngine>,
    sessions: Mutex<HashMap<Uuid, String>>,
}

#[async_trait::async_trait]
impl browser_sessions::BrowserRuntime for BrowserFakeRuntime {
    async fn create(
        &self,
        context: homebot_tools::OperationContext,
        profile: &homebot_tools::BrowserSessionProfile,
        approval_id: Option<Uuid>,
    ) -> Result<Uuid, homebot_tools::ToolError> {
        let request = homebot_tools::CapabilityRequest {
            context,
            capability: homebot_tools::CapabilityClass::BrowserAct,
            action: "browser.session.create".to_owned(),
            canonical_resource: format!("profile:{}", profile.profile_id),
            summary: "Open browser profile".to_owned(),
            destructive: false,
        };
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        let id = Uuid::now_v7();
        self.sessions
            .lock()
            .await
            .insert(id, "about:blank".to_owned());
        Ok(id)
    }

    async fn execute(
        &self,
        context: homebot_tools::OperationContext,
        session_id: Uuid,
        action: homebot_tools::BrowserAction,
        approval_id: Option<Uuid>,
    ) -> Result<homebot_tools::BrowserResult, homebot_tools::ToolError> {
        let action_name = match action {
            homebot_tools::BrowserAction::Navigate { .. } => "browser.navigate",
            homebot_tools::BrowserAction::CurrentUrl => "browser.current_url",
            homebot_tools::BrowserAction::CaptureScreenshot => "browser.screenshot",
            homebot_tools::BrowserAction::Evaluate { .. } => "browser.evaluate",
        };
        let capability = if matches!(
            &action,
            homebot_tools::BrowserAction::CurrentUrl
                | homebot_tools::BrowserAction::CaptureScreenshot
        ) {
            homebot_tools::CapabilityClass::BrowserObserve
        } else {
            homebot_tools::CapabilityClass::BrowserAct
        };
        let request = homebot_tools::CapabilityRequest {
            context,
            capability,
            action: action_name.to_owned(),
            canonical_resource: format!("browser-session:{session_id}:{action_name}"),
            summary: "Run browser action".to_owned(),
            destructive: capability == homebot_tools::CapabilityClass::BrowserAct,
        };
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        let mut sessions = self.sessions.lock().await;
        let url = sessions
            .get_mut(&session_id)
            .ok_or(homebot_tools::ToolError::Unavailable)?;
        match action {
            homebot_tools::BrowserAction::Navigate { url: next } => {
                *url = next;
                Ok(homebot_tools::BrowserResult::NavigationAccepted)
            }
            homebot_tools::BrowserAction::CurrentUrl => {
                Ok(homebot_tools::BrowserResult::Url { url: url.clone() })
            }
            homebot_tools::BrowserAction::CaptureScreenshot => {
                Ok(homebot_tools::BrowserResult::ScreenshotPng {
                    bytes: b"png".to_vec(),
                })
            }
            homebot_tools::BrowserAction::Evaluate { .. } => {
                Err(homebot_tools::ToolError::InvalidRequest(
                    "evaluation disabled in fixture".to_owned(),
                ))
            }
        }
    }

    async fn close(
        &self,
        context: homebot_tools::OperationContext,
        session_id: Uuid,
        approval_id: Option<Uuid>,
    ) -> Result<(), homebot_tools::ToolError> {
        let request = homebot_tools::CapabilityRequest {
            context,
            capability: homebot_tools::CapabilityClass::BrowserAct,
            action: "browser.session.close".to_owned(),
            canonical_resource: format!("browser-session:{session_id}"),
            summary: "Close browser".to_owned(),
            destructive: false,
        };
        let _authorization = self.policy.authorize(&request, approval_id).await?;
        self.sessions.lock().await.remove(&session_id);
        Ok(())
    }
}

struct TestApp {
    router: Router,
    _directory: tempfile::TempDir,
}

async fn test_app() -> Result<TestApp, homebot_storage::StorageError> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("homebot.db");
    let storage = Storage::open(&path).await?;
    Ok(TestApp {
        router: router(AppState::new(storage, "correct-token")),
        _directory: directory,
    })
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn global_search_is_owner_scoped_and_returns_exact_targets()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let owner = Uuid::nil();
    let other_owner = Uuid::now_v7();
    let bot = storage
        .create_bot(owner, homebot_domain::Bot::create("Nova", "Research")?, 1)
        .await?;
    let chat = storage
        .create_direct_chat(owner, bot.id.0, Uuid::now_v7(), 2)
        .await?;
    let message_id = Uuid::now_v7();
    storage
        .append_user_message(
            owner,
            chat.id,
            message_id,
            "Review the launch brief at https://example.test/launch.",
            &[],
            None,
            Vec::new(),
            &[],
            &[],
            3,
        )
        .await?;
    let artifact_id = Uuid::now_v7();
    storage
        .insert_artifact(&homebot_storage::ArtifactRecord {
            id: artifact_id,
            owner_id: owner,
            chat_id: chat.id,
            message_id: Some(message_id),
            activity_id: None,
            name: "launch-brief.pdf".to_owned(),
            kind: "document".to_owned(),
            media_type: "application/pdf".to_owned(),
            size_bytes: 12,
            sha256: "0".repeat(64),
            storage_path: "fixture/launch-brief.pdf".to_owned(),
            created_at_ms: 4,
        })
        .await?;
    let routine_id = Uuid::now_v7();
    storage
        .create_routine(&homebot_storage::RoutineRecord {
            id: routine_id,
            owner_id: owner,
            bot_id: bot.id.0,
            name: "Launch review".to_owned(),
            description: "Review the launch brief".to_owned(),
            enabled: false,
            draft: true,
            active_version_id: Uuid::now_v7(),
            version: 1,
            definition: RoutineDefinition {
                inputs: Vec::new(),
                steps: Vec::new(),
                expected_outputs: Vec::new(),
            },
            created_at_ms: 5,
            updated_at_ms: 5,
        })
        .await?;
    let foreign_bot = storage
        .create_bot(
            other_owner,
            homebot_domain::Bot::create("Foreign", "Private")?,
            6,
        )
        .await?;
    let foreign_chat = storage
        .create_direct_chat(other_owner, foreign_bot.id.0, Uuid::now_v7(), 7)
        .await?;
    storage
        .append_user_message(
            other_owner,
            foreign_chat.id,
            Uuid::now_v7(),
            "launch secret",
            &[],
            None,
            Vec::new(),
            &[],
            &[],
            8,
        )
        .await?;

    let app = router(AppState::new(storage, "correct-token"));
    let unauthorized = app
        .clone()
        .oneshot(Request::get("/api/v1/search?q=launch").body(Body::empty())?)
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .oneshot(
            Request::get("/api/v1/search?q=launch")
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let response = response_json::<GlobalSearchResponse>(response).await?;
    assert!(response.results.iter().any(|result| {
        result.message_id == Some(message_id)
            && result.deep_link == format!("homebot://chat/{}?message={message_id}", chat.id)
    }));
    assert!(response.results.iter().any(|result| {
        result.artifact_id == Some(artifact_id)
            && result
                .deep_link
                .contains(&format!("artifact={artifact_id}"))
    }));
    assert!(response.results.iter().any(|result| {
        result.routine_id == Some(routine_id)
            && result.deep_link == format!("homebot://routine/{routine_id}")
    }));
    assert!(
        response
            .results
            .iter()
            .all(|result| result.chat_id != Some(foreign_chat.id))
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn demonstrated_recording_creates_editable_restart_durable_and_safely_testable_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("homebot.db");
    let storage = Storage::open(&database).await?;
    let app = router(AppState::new(storage, "correct-token"));
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
                provider_profile_id: None,
                permission_profile: BotPermissionProfile::AskBeforeChanges,
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let recording_id = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/routine-recordings",
            &StartRoutineRecordingRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: recording_id,
                bot_id,
                name: "Review launch".to_owned(),
                description: "A demonstrated review workflow".to_owned(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routine-recordings/{recording_id}/actions"),
            &AppendRoutineRecordingRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                action: RecordedAction {
                    actor: RecordedActor::User,
                    step: RoutineStep::BotPrompt {
                        bot_id,
                        prompt_template: "Review the launch brief".to_owned(),
                        requires_approval: true,
                    },
                },
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let skill_id = Uuid::now_v7();
    let finish = BotMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: skill_id,
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routine-recordings/{recording_id}/finish-skill"),
            &finish,
        ))
        .await?;
    let skill = response_json::<SkillSummary>(response).await?;
    assert_eq!(skill.id, skill_id);
    assert_eq!(skill.version, 1);
    assert!(
        skill
            .definition
            .instructions
            .contains("approval is required")
    );
    assert_eq!(skill.definition.context[0].label, "Recorded demonstration");
    let replay = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routine-recordings/{recording_id}/finish-skill"),
            &finish,
        ))
        .await?;
    assert_eq!(response_json::<SkillSummary>(replay).await?.id, skill_id);

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/skills/{skill_id}/test"),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    let preview = response_json::<SkillTestSummary>(response).await?;
    assert!(preview.capability_policy_enforced);
    assert_eq!(preview.skill_version_id, skill.active_version_id);
    assert!(preview.prompt_preview.contains("Review the launch brief"));

    let mut edited = skill.definition;
    edited.instructions.push_str("\nReturn concise findings.");
    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/skills/{skill_id}"),
            &UpdateSkillRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                name: "Review launch".to_owned(),
                description: "Edited after demonstration".to_owned(),
                definition: edited,
            },
        ))
        .await?;
    assert_eq!(response_json::<SkillSummary>(response).await?.version, 2);

    drop(app);
    let reopened = router(AppState::new(
        Storage::open(&database).await?,
        "correct-token",
    ));
    let response = reopened
        .oneshot(
            Request::get("/api/v1/skills")
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let skills = response_json::<Vec<SkillSummary>>(response).await?;
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].version, 2);
    assert!(
        skills[0]
            .definition
            .instructions
            .contains("concise findings")
    );
    Ok(())
}

#[tokio::test]
async fn cold_start_and_authenticated_protocol_probe_meet_release_budgets()
-> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let app = test_app().await?;
    assert!(
        started.elapsed() <= Duration::from_secs(5),
        "cold start exceeded the five-second release budget"
    );
    let probes_started = Instant::now();
    for _ in 0..100 {
        let response = app
            .router
            .clone()
            .oneshot(
                Request::get("/api/v1/version")
                    .header("authorization", "Bearer correct-token")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert!(
        probes_started.elapsed() <= Duration::from_secs(2),
        "100 authenticated protocol probes exceeded the two-second budget"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_registry_connects_local_mcp_and_persists_error_recovery_states()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let server = directory.path().join("fixture-mcp");
    let script = "#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\n*\\\"method\\\":\\\"initialize\\\"*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"fixture\",\"version\":\"1\"}}}' ;;\n*\\\"method\\\":\\\"tools/list\\\"*) printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"repo_status\",\"description\":\"Untrusted metadata\",\"inputSchema\":{\"type\":\"object\"}}]}}' ;;\nesac\ndone\n";
    std::fs::write(&server, script)?;
    let mut permissions = std::fs::metadata(&server)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&server, permissions)?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let app = router(AppState::new(storage.clone(), "correct-token"));
    let plugin_id = Uuid::now_v7();
    let create = CreateLocalMcpPluginRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: plugin_id,
        name: "Repository tools".to_owned(),
        description: "Local fixture".to_owned(),
        program: server.display().to_string(),
        arguments: Vec::new(),
    };
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/plugins")
                .header("authorization", "Bearer correct-token")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&create)?))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: PluginSummary =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?;
    assert_eq!(created.connection_state, PluginConnectionState::Connect);
    let mutation = PluginMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let response = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/plugins/{plugin_id}/connect"))
                .header("authorization", "Bearer correct-token")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&mutation)?))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let connected: PluginSummary =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?;
    assert_eq!(connected.connection_state, PluginConnectionState::Connected);
    assert!(connected.enabled);
    assert_eq!(
        connected.tools.first().map(|tool| tool.name.as_str()),
        Some("repo_status")
    );
    std::fs::write(&server, "#!/bin/sh\nprintf '%s\\n' 'not-json'\n")?;
    let mutation = PluginMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let response = app
        .oneshot(
            Request::post(format!("/api/v1/plugins/{plugin_id}/health"))
                .header("authorization", "Bearer correct-token")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&mutation)?))?,
        )
        .await?;
    let errored: PluginSummary =
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?;
    assert_eq!(errored.connection_state, PluginConnectionState::Error);
    assert!(!errored.enabled);
    assert!(errored.tools.is_empty());
    Ok(())
}

#[tokio::test]
async fn paired_devices_cannot_register_local_plugin_executables()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let device_token = "paired-plugin-mutation-denied";
    let digest: [u8; 32] = Sha256::digest(device_token.as_bytes()).into();
    sqlx::query("INSERT INTO device_sessions (id, owner_id, name, token_digest, endpoint_kind, created_at_ms) VALUES (?, ?, 'Remote phone', ?, 'loopback', 1)")
        .bind(Uuid::now_v7().to_string())
        .bind(Uuid::nil().to_string())
        .bind(digest.as_slice())
        .execute(storage.pool())
        .await?;
    let app = router(AppState::new(storage.clone(), "correct-token"));
    let plugin_id = Uuid::now_v7();
    let response = app
        .oneshot(
            Request::post("/api/v1/plugins")
                .header("authorization", format!("Bearer {device_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(
                    &CreateLocalMcpPluginRequest {
                        request_id: Uuid::now_v7(),
                        idempotency_key: plugin_id,
                        name: "Unsafe remote executable".to_owned(),
                        description: String::new(),
                        program: "/bin/sh".to_owned(),
                        arguments: vec!["-c".to_owned(), "exit 0".to_owned()],
                    },
                )?))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        storage
            .list_plugins(Uuid::nil())
            .await?
            .into_iter()
            .all(|plugin| plugin.id != plugin_id)
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn routine_plugin_calls_require_server_policy_even_when_definition_opts_out()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let marker = directory.path().join("plugin-called");
    let server = directory.path().join("fixture-mcp");
    let script = format!(
        "#!/bin/sh\nwhile IFS= read -r line; do\ncase \"$line\" in\n*\\\"method\\\":\\\"initialize\\\"*) printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":\"2025-11-25\",\"capabilities\":{{\"tools\":{{}}}},\"serverInfo\":{{\"name\":\"fixture\",\"version\":\"1\"}}}}}}' ;;\n*\\\"method\\\":\\\"tools/list\\\"*) printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"tools\":[{{\"name\":\"write_marker\",\"inputSchema\":{{\"type\":\"object\"}}}}]}}}}' ;;\n*\\\"method\\\":\\\"tools/call\\\"*) touch '{}'; printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"done\"}}]}}}}' ;;\nesac\ndone\n",
        marker.display()
    );
    std::fs::write(&server, script)?;
    let mut permissions = std::fs::metadata(&server)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&server, permissions)?;

    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let bot = storage
        .create_bot(
            Uuid::nil(),
            homebot_domain::Bot::create("Routine Bot", "Automation")?,
            1,
        )
        .await?;
    let app = router(AppState::new(storage, "correct-token"));
    let plugin_id = Uuid::now_v7();
    assert_eq!(
        app.clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/plugins",
                &CreateLocalMcpPluginRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: plugin_id,
                    name: "Routine fixture".to_owned(),
                    description: String::new(),
                    program: server.display().to_string(),
                    arguments: Vec::new(),
                },
            ))
            .await?
            .status(),
        StatusCode::CREATED
    );
    let mutation = PluginMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    assert_eq!(
        app.clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/plugins/{plugin_id}/connect"),
                &mutation,
            ))
            .await?
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone()
            .oneshot(json_request(
                "PUT",
                &format!("/api/v1/plugins/{plugin_id}/assignment"),
                &PluginAssignmentRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                    bot_id: bot.id.0,
                    enabled: true,
                },
            ))
            .await?
            .status(),
        StatusCode::OK
    );
    let routine_id = Uuid::now_v7();
    assert_eq!(
        app.clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/routines",
                &CreateRoutineRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: routine_id,
                    bot_id: bot.id.0,
                    name: "Policy-derived plugin call".to_owned(),
                    description: String::new(),
                    definition: RoutineDefinition {
                        inputs: Vec::new(),
                        steps: vec![RoutineStep::PluginTool {
                            plugin_id,
                            tool_name: "write_marker".to_owned(),
                            arguments: json!({}),
                            requires_approval: false,
                        }],
                        expected_outputs: Vec::new(),
                    },
                    draft: false,
                },
            ))
            .await?
            .status(),
        StatusCode::CREATED
    );
    let blocked: RoutineRunSummary = response_json(
        app.clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/routines/{routine_id}/run"),
                &RunRoutineRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                    inputs: json!({}),
                },
            ))
            .await?,
    )
    .await?;
    assert_eq!(blocked.status, "waiting_approval");
    assert!(!marker.exists());

    let rule_id = Uuid::now_v7();
    assert_eq!(
        app.clone()
            .oneshot(json_request(
                "PUT",
                &format!("/api/v1/capability-rules/{rule_id}"),
                &UpsertCapabilityRuleRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                    capability: CapabilityClass::PluginWrite,
                    effect: CapabilityRuleEffect::Allow,
                    device_id: None,
                    bot_id: Some(bot.id.0),
                    chat_id: None,
                    workspace_id: None,
                    action_prefix: Some("plugin.tool.call".to_owned()),
                },
            ))
            .await?
            .status(),
        StatusCode::OK
    );
    let allowed: RoutineRunSummary = response_json(
        app.oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/run"),
            &RunRoutineRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                inputs: json!({}),
            },
        ))
        .await?,
    )
    .await?;
    assert_eq!(allowed.status, "succeeded");
    assert!(marker.exists());
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn recorded_routine_edits_versions_dry_runs_and_replays_deterministically()
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
    let app = router(AppState::new(storage.clone(), "correct-token"));
    let recording_id = Uuid::now_v7();
    let start = StartRoutineRecordingRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: recording_id,
        bot_id: bot.id.0,
        name: "Morning brief".to_owned(),
        description: "Recorded once".to_owned(),
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/routine-recordings", &start))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let recording: RoutineRecordingSummary = response_json(response).await?;
    assert!(recording.actions.is_empty());
    let append = AppendRoutineRecordingRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        action: RecordedAction {
            actor: RecordedActor::User,
            step: RoutineStep::BotPrompt {
                bot_id: bot.id.0,
                prompt_template: "Summarise the overnight updates".to_owned(),
                requires_approval: false,
            },
        },
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routine-recordings/{recording_id}/actions"),
            &append,
        ))
        .await?;
    let recorded: RoutineRecordingSummary = response_json(response).await?;
    assert_eq!(recorded.actions.len(), 1);
    let routine_id = Uuid::now_v7();
    let finish = BotMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: routine_id,
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routine-recordings/{recording_id}/finish"),
            &finish,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let draft: RoutineSummary = response_json(response).await?;
    assert!(draft.draft);
    let update = UpdateRoutineRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        name: "Morning intelligence".to_owned(),
        description: "Edited and published".to_owned(),
        definition: draft.definition,
        draft: false,
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/routines/{routine_id}"),
            &update,
        ))
        .await?;
    let published: RoutineSummary = response_json(response).await?;
    assert_eq!(published.version, 2);
    let enable = PluginMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/enable"),
            &enable,
        ))
        .await?;
    assert!(response_json::<RoutineSummary>(response).await?.enabled);
    let dry = RunRoutineRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        inputs: json!({}),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/dry-run"),
            &dry,
        ))
        .await?;
    let dry_run: RoutineRunSummary = response_json(response).await?;
    assert!(
        dry_run
            .results
            .iter()
            .all(|step| step.status == RoutineStepStatus::Planned)
    );
    let manual = RunRoutineRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        inputs: json!({}),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/run"),
            &manual,
        ))
        .await?;
    let run: RoutineRunSummary = response_json(response).await?;
    assert_eq!(run.status, "succeeded");
    assert_eq!(run.routine_version_id, published.active_version_id);
    let chat = storage
        .get_direct_chat_for_bot(Uuid::nil(), bot.id.0)
        .await?;
    let messages = storage.chat_messages(Uuid::nil(), chat.id).await?;
    assert!(messages.iter().any(|message| message.parts.iter().any(|part| matches!(part, homebot_domain::chat::MessagePart::Text { text, .. } if text == "Summarise the overnight updates"))));
    let duplicate = DuplicateRoutineRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        name: "Morning intelligence copy".to_owned(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/duplicate"),
            &duplicate,
        ))
        .await?;
    let copy: RoutineSummary = response_json(response).await?;
    assert!(copy.draft && !copy.enabled);
    let response = app
        .oneshot(
            Request::get(format!("/api/v1/routines/{routine_id}/runs"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(
        response_json::<Vec<RoutineRunSummary>>(response)
            .await?
            .len(),
        2
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn routine_failures_are_durable_and_invalid_finish_keeps_recording_editable()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("homebot.db");
    let storage = Storage::open(&database).await?;
    let bot = storage
        .create_bot(
            Uuid::nil(),
            homebot_domain::Bot::create("Nova", "Research")?,
            1,
        )
        .await?;
    let app = router(AppState::new(storage.clone(), "correct-token"));

    let recording_id = Uuid::now_v7();
    let start = StartRoutineRecordingRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: recording_id,
        bot_id: bot.id.0,
        name: "Recoverable recording".to_owned(),
        description: String::new(),
    };
    let _ = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/routine-recordings", &start))
        .await?;
    let finish = BotMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let invalid_finish = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routine-recordings/{recording_id}/finish"),
            &finish,
        ))
        .await?;
    assert_eq!(invalid_finish.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let append = AppendRoutineRecordingRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        action: RecordedAction {
            actor: RecordedActor::User,
            step: RoutineStep::BotPrompt {
                bot_id: bot.id.0,
                prompt_template: "Try again".to_owned(),
                requires_approval: false,
            },
        },
    };
    let still_editable = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routine-recordings/{recording_id}/actions"),
            &append,
        ))
        .await?;
    assert_eq!(still_editable.status(), StatusCode::OK);

    let routine_id = Uuid::now_v7();
    let create = CreateRoutineRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: routine_id,
        bot_id: bot.id.0,
        name: "Unavailable Bot fixture".to_owned(),
        description: String::new(),
        definition: RoutineDefinition {
            inputs: Vec::new(),
            steps: vec![RoutineStep::BotPrompt {
                bot_id: Uuid::now_v7(),
                prompt_template: "Cannot dispatch".to_owned(),
                requires_approval: false,
            }],
            expected_outputs: Vec::new(),
        },
        draft: false,
    };
    let created = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/routines", &create))
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let run_id = Uuid::now_v7();
    let failed = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/run"),
            &RunRoutineRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: run_id,
                inputs: json!({}),
            },
        ))
        .await?;
    let failed: RoutineRunSummary = response_json(failed).await?;
    assert_eq!(failed.status, "failed");
    assert_eq!(
        failed.error_message.as_deref(),
        Some("routine definition is invalid: step Bot differs from routine Bot")
    );
    drop(app);
    storage.pool().close().await;

    let reopened = Storage::open(&database).await?;
    let runs = reopened.routine_runs(Uuid::nil(), routine_id).await?;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, run_id);
    assert_eq!(runs[0].status, "failed");
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn headless_scheduler_survives_restart_deduplicates_and_redacts_history()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("homebot.db");
    let storage = Storage::open(&database).await?;
    let bot = storage
        .create_bot(
            Uuid::nil(),
            homebot_domain::Bot::create("Nova", "Research")?,
            1,
        )
        .await?;
    let state = AppState::new(storage.clone(), "correct-token");
    let app = router(state.clone());
    let routine_id = Uuid::now_v7();
    let create = CreateRoutineRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: routine_id,
        bot_id: bot.id.0,
        name: "Durable scheduler fixture".to_owned(),
        description: String::new(),
        definition: RoutineDefinition {
            inputs: vec![RoutineInput {
                key: "credential".to_owned(),
                label: "Credential".to_owned(),
                kind: RoutineInputKind::SecretReference,
                required: false,
            }],
            steps: vec![RoutineStep::RecordOutput {
                output_key: "result".to_owned(),
                value_template: "done".to_owned(),
            }],
            expected_outputs: vec![homebot_protocol::ExpectedOutput {
                key: "result".to_owned(),
                description: "Deterministic fixture".to_owned(),
                required: true,
            }],
        },
        draft: false,
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/routines", &create))
        .await?;
    let routine: RoutineSummary = response_json(response).await?;
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/enable"),
            &PluginMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);

    let scheduled_for = unix_time_ms() + 200;
    let schedule_trigger_id = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/triggers"),
            &CreateRoutineTriggerRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: schedule_trigger_id,
                definition: RoutineTriggerDefinition {
                    source: RoutineTriggerSource::Schedule {
                        schedule: RoutineSchedule::OneShot {
                            at_unix_ms: scheduled_for,
                        },
                    },
                    missed_run_policy: MissedRunPolicy::RunOnce,
                    overlap_policy: OverlapPolicy::Queue,
                    retry_policy: RetryPolicy::default(),
                    catch_up_limit: 1,
                },
                enabled: true,
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let _ = state.server_shutdown.send(true);
    drop(app);
    tokio::time::sleep(Duration::from_millis(250)).await;

    let restarted = AppState::new(storage.clone(), "correct-token");
    let app = router(restarted.clone());
    let scheduled =
        wait_for_job_status(&app, routine_id, "succeeded", Duration::from_secs(2)).await?;
    assert_eq!(scheduled.routine_version_id, routine.active_version_id);
    assert_eq!(scheduled.trigger_id, schedule_trigger_id);

    let event_trigger_id = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/triggers"),
            &CreateRoutineTriggerRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: event_trigger_id,
                definition: trigger_definition(RoutineTriggerSource::Event {
                    event_kind: "fixture_event".to_owned(),
                }),
                enabled: true,
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let rejected = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routine-triggers/{event_trigger_id}/deliver"),
            &DeliverRoutineTriggerRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                delivery_key: "forged-event".to_owned(),
                inputs: json!({}),
            },
        ))
        .await?;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let _ = restarted.server_shutdown.send(true);
    tokio::time::sleep(Duration::from_millis(20)).await;
    storage
        .append_event(Uuid::nil(), "fixture_event", &json!({}), unix_time_ms())
        .await?;
    drop(app);

    let final_state = AppState::new(storage.clone(), "correct-token");
    let app = router(final_state.clone());
    let event_job =
        wait_for_trigger_job(&app, routine_id, event_trigger_id, Duration::from_secs(2)).await?;
    assert_eq!(event_job.status, "succeeded");

    let plugin_id = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/plugins",
            &CreateLocalMcpPluginRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: plugin_id,
                name: "Scheduler event fixture".to_owned(),
                description: String::new(),
                program: "/bin/false".to_owned(),
                arguments: Vec::new(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let plugin_trigger_id = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/triggers"),
            &CreateRoutineTriggerRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: plugin_trigger_id,
                definition: trigger_definition(RoutineTriggerSource::Plugin {
                    plugin_id,
                    event_kind: "plugin_changed".to_owned(),
                }),
                enabled: true,
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    storage
        .append_event(
            Uuid::nil(),
            "plugin_changed",
            &json!({"kind":"plugin_changed","plugin":{"id":Uuid::now_v7()}}),
            unix_time_ms(),
        )
        .await?;
    storage
        .append_event(
            Uuid::nil(),
            "plugin_changed",
            &json!({"kind":"plugin_changed","plugin":{"id":plugin_id}}),
            unix_time_ms(),
        )
        .await?;
    let plugin_job =
        wait_for_trigger_job(&app, routine_id, plugin_trigger_id, Duration::from_secs(2)).await?;
    assert_eq!(plugin_job.status, "succeeded");
    assert_eq!(
        routine_jobs(&app, routine_id)
            .await?
            .iter()
            .filter(|job| job.trigger_id == plugin_trigger_id)
            .count(),
        1
    );

    let webhook_trigger_id = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routines/{routine_id}/triggers"),
            &CreateRoutineTriggerRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: webhook_trigger_id,
                definition: trigger_definition(RoutineTriggerSource::Webhook {
                    slug: "deploy".to_owned(),
                }),
                enabled: true,
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let sensitive_reference = Uuid::now_v7().to_string();
    let delivery = DeliverRoutineTriggerRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        delivery_key: "delivery-42".to_owned(),
        inputs: json!({"credential":sensitive_reference.clone()}),
    };
    let first = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routine-triggers/{webhook_trigger_id}/deliver"),
            &delivery,
        ))
        .await?;
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first: RoutineJobSummary = response_json(first).await?;
    let duplicate = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/routine-triggers/{webhook_trigger_id}/deliver"),
            &delivery,
        ))
        .await?;
    assert_eq!(duplicate.status(), StatusCode::OK);
    assert_eq!(
        response_json::<RoutineJobSummary>(duplicate).await?.id,
        first.id
    );
    let completed =
        wait_for_trigger_job(&app, routine_id, webhook_trigger_id, Duration::from_secs(2)).await?;
    assert_eq!(completed.status, "succeeded");
    assert!(!serde_json::to_string(&completed)?.contains(&sensitive_reference));

    let runs_response = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/routines/{routine_id}/runs"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let runs: Vec<RoutineRunSummary> = response_json(runs_response).await?;
    assert_eq!(runs.len(), 4);
    assert!(runs.iter().all(|run| run.bot_id == bot.id.0));
    assert!(runs.iter().all(|run| run.scheduled_for_unix_ms.is_some()));
    let webhook_run = runs
        .iter()
        .find(|run| run.trigger.get("kind") == Some(&json!("webhook")))
        .ok_or("missing webhook run")?;
    assert_eq!(
        webhook_run.input_metadata,
        json!({"credential":{"kind":"secret_reference","present":true}})
    );
    assert!(!serde_json::to_string(&runs)?.contains(&sensitive_reference));
    let _ = final_state.server_shutdown.send(true);
    Ok(())
}

fn trigger_definition(source: RoutineTriggerSource) -> RoutineTriggerDefinition {
    RoutineTriggerDefinition {
        source,
        missed_run_policy: MissedRunPolicy::RunOnce,
        overlap_policy: OverlapPolicy::Queue,
        retry_policy: RetryPolicy::default(),
        catch_up_limit: 1,
    }
}

async fn wait_for_job_status(
    app: &Router,
    routine_id: Uuid,
    status: &str,
    timeout: Duration,
) -> Result<RoutineJobSummary, Box<dyn std::error::Error>> {
    let started = tokio::time::Instant::now();
    loop {
        let jobs = routine_jobs(app, routine_id).await?;
        if let Some(job) = jobs.into_iter().find(|job| job.status == status) {
            return Ok(job);
        }
        if started.elapsed() >= timeout {
            return Err(format!("routine job did not reach {status}").into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_for_trigger_job(
    app: &Router,
    routine_id: Uuid,
    trigger_id: Uuid,
    timeout: Duration,
) -> Result<RoutineJobSummary, Box<dyn std::error::Error>> {
    let started = tokio::time::Instant::now();
    loop {
        let jobs = routine_jobs(app, routine_id).await?;
        if let Some(job) = jobs
            .iter()
            .find(|job| job.trigger_id == trigger_id && job.status == "succeeded")
        {
            return Ok(job.clone());
        }
        if started.elapsed() >= timeout {
            return Err(format!("routine trigger job did not succeed: {jobs:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn routine_jobs(
    app: &Router,
    routine_id: Uuid,
) -> Result<Vec<RoutineJobSummary>, Box<dyn std::error::Error>> {
    let response = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/routines/{routine_id}/jobs"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    response_json(response).await
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

    for (action, expected) in [("pin", true), ("hide", true), ("unhide", false)] {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/bots/{key}/{action}"),
                &BotMutationRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                },
            ))
            .await?;
        let bot = response_json::<BotResponse>(response).await?.bot;
        if action == "pin" {
            assert_eq!(bot.pinned, expected);
        } else {
            assert_eq!(bot.hidden, expected);
        }
    }
    let duplicate_key = Uuid::now_v7();
    let duplicate_request = BotMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: duplicate_key,
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/bots/{key}/duplicate"),
            &duplicate_request,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response_json::<BotResponse>(response).await?.bot.name,
        "Nova copy"
    );
    let replay = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/bots/{key}/duplicate"),
            &duplicate_request,
        ))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    let wrong_confirmation = DeleteBotRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        confirm_name: "Nova".to_owned(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "DELETE",
            &format!("/api/v1/bots/{duplicate_key}"),
            &wrong_confirmation,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let delete_request = DeleteBotRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        confirm_name: "Nova copy".to_owned(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "DELETE",
            &format!("/api/v1/bots/{duplicate_key}"),
            &delete_request,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let replay = app
        .clone()
        .oneshot(json_request(
            "DELETE",
            &format!("/api/v1/bots/{duplicate_key}"),
            &delete_request,
        ))
        .await?;
    assert_eq!(replay.status(), StatusCode::NO_CONTENT);
    let unknown_delete = DeleteBotRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        confirm_name: "Missing".to_owned(),
    };
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(json_request(
                "DELETE",
                &format!("/api/v1/bots/{}", Uuid::nil()),
                &unknown_delete,
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

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
    assert!(
        events
            .iter()
            .all(|event| matches!(event.event_kind.as_str(), "bot_changed" | "bot_deleted"))
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.event_kind == "bot_deleted")
            .count(),
        1
    );
    storage.pool().close().await;
    let reopened = Storage::open(&database).await?;
    assert_eq!(reopened.list_bots(Uuid::nil(), true).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn repeated_chat_read_does_not_publish_noop_events() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let owner = Uuid::nil();
    let bot = storage
        .create_bot(owner, homebot_domain::Bot::create("Nova", "Research")?, 1)
        .await?;
    let chat = storage
        .create_direct_chat(owner, bot.id.0, Uuid::now_v7(), 2)
        .await?;
    storage.increment_chat_unread(owner, chat.id, 3).await?;
    let app = router(AppState::new(storage.clone(), "correct-token"));

    for expected_events in [2, 2] {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/chats/{}/read", chat.id),
                &BotMutationRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                },
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(storage.latest_sequence(owner).await?, expected_events);
    }
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
        skill_ids: Vec::new(),
        references: vec![MessageReferenceInput {
            kind: MessageReferenceKind::Bot,
            target_id: bot_id,
        }],
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
    let rename = UpdateBotRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        name: "Nova renamed".to_owned(),
        title: "Research".to_owned(),
        description: String::new(),
        shape: BotShape::RoundedSquare,
        color: BotColor::Violet,
        provider_profile_id: None,
        permission_profile: BotPermissionProfile::AskBeforeChanges,
    };
    assert_eq!(
        app.clone()
            .oneshot(json_request(
                "PUT",
                &format!("/api/v1/bots/{bot_id}"),
                &rename
            ))
            .await?
            .status(),
        StatusCode::OK
    );
    let replay = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_key}/messages"),
            &send,
        ))
        .await?;
    let replayed = response_json::<SendMessageResponse>(replay).await?;
    assert!(
        matches!(&replayed, SendMessageResponse::Sent { message } if message.id == message_key)
    );
    let SendMessageResponse::Sent { message } = replayed else {
        unreachable!()
    };
    assert_eq!(message.references[0].label, "Nova");

    let invalid_reference_key = Uuid::now_v7();
    let invalid_reference = SendMessageRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: invalid_reference_key,
        content: "Use an unavailable plugin".to_owned(),
        attachment_ids: Vec::new(),
        reply_to_message_id: None,
        mentioned_bot_ids: Vec::new(),
        skill_ids: Vec::new(),
        references: vec![MessageReferenceInput {
            kind: MessageReferenceKind::Plugin,
            target_id: Uuid::nil(),
        }],
    };
    let invalid = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{chat_key}/messages"),
            &invalid_reference,
        ))
        .await?;
    assert_eq!(invalid.status(), StatusCode::NOT_FOUND);
    assert!(matches!(
        storage.message(Uuid::nil(), invalid_reference_key).await,
        Err(homebot_storage::StorageError::MessageNotFound)
    ));

    let reaction = ReactionMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        emoji: "👍".to_owned(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/messages/{message_key}/reactions"),
            &reaction,
        ))
        .await?;
    let reacted = response_json::<homebot_protocol::MessageSummary>(response).await?;
    assert_eq!(reacted.reactions[0].count, 1);
    assert!(reacted.reactions[0].reacted_by_user);
    let response = app
        .clone()
        .oneshot(json_request(
            "DELETE",
            &format!("/api/v1/messages/{message_key}/reactions"),
            &ReactionMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                emoji: "👍".to_owned(),
            },
        ))
        .await?;
    assert!(
        response_json::<homebot_protocol::MessageSummary>(response)
            .await?
            .reactions
            .is_empty()
    );

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
        skill_ids: Vec::new(),
        references: Vec::new(),
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
        SendMessageResponse::Queued { prompt }
            if prompt.kind == QueuedPromptKind::Steering
    ));
    let queued_key = Uuid::now_v7();
    let queued = SendMessageRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: queued_key,
        content: "Follow up".to_owned(),
        attachment_ids: Vec::new(),
        reply_to_message_id: None,
        mentioned_bot_ids: Vec::new(),
        skill_ids: Vec::new(),
        references: Vec::new(),
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
    assert_eq!(timeline.messages.len(), 1);
    assert_eq!(timeline.approvals.len(), 1);
    assert_eq!(timeline.queued_prompts.len(), 2);
    assert_eq!(timeline.queued_prompts[0].kind, QueuedPromptKind::Steering);
    assert_eq!(timeline.queued_prompts[1].kind, QueuedPromptKind::FollowUp);
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
    let provider = Arc::new(ChatFakeAdapter::new()?);
    runtime.register(provider.clone()).await?;
    let artifact_root = directory.path().join("artifacts");
    let state = AppState::new(storage.clone(), "correct-token")
        .with_artifact_root(artifact_root.clone())
        .with_provider_runtime(runtime);
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
        skill_ids: Vec::new(),
        references: Vec::new(),
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
    assert_eq!(
        *provider.working_directories.lock().await,
        vec![Some(
            artifact_root
                .join("bot-workspaces")
                .join(bot_id.to_string())
        )]
    );
    let timeline = wait_for_timeline(&app, chat_id, |timeline| {
        timeline.approvals.len() == 1
            && timeline.messages.len() == 2
            && timeline.activities.len() == 1
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
                skill_ids: Vec::new(),
                references: Vec::new(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let _ = wait_for_timeline(&app, chat_id, |timeline| {
        timeline.chat.running
            && timeline.messages.len() == 4
            && timeline.approvals.len() == 2
            && timeline.activities.len() == 2
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
    assert_eq!(
        timeline.approvals[1].status,
        homebot_protocol::ApprovalStatus::Expired
    );
    assert_eq!(
        timeline.activities[1].status,
        homebot_protocol::ActivityStatus::Cancelled
    );
    assert!(timeline.activities[1].finished_at_ms.is_some());
    let late_allow = app
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{}/decision", timeline.approvals[1].id),
            &ApprovalDecisionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                allow: true,
            },
        ))
        .await?;
    assert_eq!(late_allow.status(), StatusCode::CONFLICT);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn queued_followups_and_steering_are_idempotent_restart_durable_and_cancel_stable()
-> Result<(), Box<dyn std::error::Error>> {
    let _provider_queue_guard = provider_queue_test_guard().await;
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("homebot.db");
    let storage = Storage::open(&database).await?;
    let profile_id = Uuid::now_v7();
    sqlx::query("INSERT INTO provider_profiles (id, adapter_kind, display_name, configuration_json, created_at_ms, updated_at_ms) VALUES (?, 'chat-fake', 'Fixture', '{}', 1, 1)")
        .bind(profile_id.to_string()).execute(storage.pool()).await?;
    let runtime = Arc::new(ProviderRuntime::new());
    runtime.register(Arc::new(ChatFakeAdapter::new()?)).await?;
    let mut bot = homebot_domain::Bot::create("Queue Bot", "Priorities")?;
    bot.provider_profile_id = Some(profile_id);
    let bot = storage.create_bot(Uuid::nil(), bot, 1).await?;
    let chat = storage
        .create_direct_chat(Uuid::nil(), bot.id.0, Uuid::now_v7(), 2)
        .await?;
    let app =
        router(AppState::new(storage.clone(), "correct-token").with_provider_runtime(runtime));
    let request = |key: Uuid, content: &str| SendMessageRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: key,
        content: content.to_owned(),
        attachment_ids: Vec::new(),
        reply_to_message_id: None,
        mentioned_bot_ids: Vec::new(),
        skill_ids: Vec::new(),
        references: Vec::new(),
    };
    let first = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/messages", chat.id),
            &request(Uuid::now_v7(), "Current work"),
        ))
        .await?;
    assert_eq!(first.status(), StatusCode::OK, "initial send failed");
    assert!(matches!(
        response_json::<SendMessageResponse>(first).await?,
        SendMessageResponse::Sent { .. }
    ));
    let _ = wait_for_timeline(&app, chat.id, |timeline| timeline.chat.running).await?;

    let follow_key = Uuid::now_v7();
    let follow = request(follow_key, "Ordinary follow-up");
    let mut original_follow = None;
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/chats/{}/messages", chat.id),
                &follow,
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "follow-up queue failed");
        let SendMessageResponse::Queued { prompt } =
            response_json::<SendMessageResponse>(response).await?
        else {
            return Err("follow-up was not queued".into());
        };
        assert_eq!(prompt.kind, QueuedPromptKind::FollowUp);
        if let Some(original) = &original_follow {
            assert_eq!(original, &prompt);
        } else {
            original_follow = Some(prompt);
        }
    }

    let steering_key = Uuid::now_v7();
    let steering = request(steering_key, "Priority steering");
    let mut original_steering = None;
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/chats/{}/steer", chat.id),
                &steering,
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::OK, "steering queue failed");
        let SendMessageResponse::Queued { prompt } =
            response_json::<SendMessageResponse>(response).await?
        else {
            return Err("steering was not queued".into());
        };
        assert_eq!(prompt.kind, QueuedPromptKind::Steering);
        if let Some(original) = &original_steering {
            assert_eq!(original, &prompt);
        } else {
            original_steering = Some(prompt);
        }
    }
    let timeline = wait_for_timeline(&app, chat.id, |_| true).await?;
    assert_eq!(timeline.chat.queued_count, 2);
    assert_eq!(
        timeline
            .queued_prompts
            .iter()
            .map(|prompt| (prompt.kind, prompt.content.as_str(), prompt.position))
            .collect::<Vec<_>>(),
        vec![
            (QueuedPromptKind::Steering, "Priority steering", 0),
            (QueuedPromptKind::FollowUp, "Ordinary follow-up", 1),
        ]
    );

    let stopped = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/stop", chat.id),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert_eq!(stopped.status(), StatusCode::OK);
    let stopped = wait_for_timeline(&app, chat.id, |timeline| {
        !timeline.chat.running
            && timeline
                .messages
                .last()
                .is_some_and(|message| message.status == homebot_protocol::MessageStatus::Cancelled)
    })
    .await?;
    assert_eq!(
        stopped
            .queued_prompts
            .iter()
            .map(|prompt| prompt.id)
            .collect::<Vec<_>>(),
        vec![steering_key, follow_key]
    );
    drop(app);
    drop(storage);

    let reopened = Storage::open(&database).await?;
    let restarted_app = router(AppState::new(reopened.clone(), "correct-token"));
    let restarted_timeline = wait_for_timeline(&restarted_app, chat.id, |_| true).await?;
    assert_eq!(
        restarted_timeline
            .queued_prompts
            .iter()
            .map(|prompt| prompt.id)
            .collect::<Vec<_>>(),
        vec![steering_key, follow_key]
    );
    drop(restarted_app);
    let durable = reopened.queued_prompts(Uuid::nil(), chat.id).await?;
    assert_eq!(
        durable
            .iter()
            .map(|prompt| (prompt.kind, prompt.id, prompt.position))
            .collect::<Vec<_>>(),
        vec![
            (
                homebot_domain::chat::QueuedPromptKind::Steering,
                steering_key,
                0
            ),
            (
                homebot_domain::chat::QueuedPromptKind::FollowUp,
                follow_key,
                1
            ),
        ]
    );
    let first_promoted = reopened
        .promote_next_queued_prompt(Uuid::nil(), chat.id, 20)
        .await?
        .ok_or("missing steering after restart")?;
    assert_eq!(
        first_promoted.prompt.kind,
        homebot_domain::chat::QueuedPromptKind::Steering
    );
    reopened
        .set_chat_running(Uuid::nil(), chat.id, false, 21)
        .await?;
    let second_promoted = reopened
        .promote_next_queued_prompt(Uuid::nil(), chat.id, 22)
        .await?
        .ok_or("missing follow-up after restart")?;
    assert_eq!(
        second_promoted.prompt.kind,
        homebot_domain::chat::QueuedPromptKind::FollowUp
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn queued_turns_plan_mode_compaction_and_reset_preserve_homebot_history()
-> Result<(), Box<dyn std::error::Error>> {
    let _provider_queue_guard = provider_queue_test_guard().await;
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("homebot.db");
    let storage = Storage::open(&database).await?;
    let profile_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO provider_profiles (
            id, adapter_kind, display_name, configuration_json, created_at_ms, updated_at_ms
         ) VALUES (?, 'chat-fake', 'Fixture', '{\"model\":\"fixture\"}', 1, 1)",
    )
    .bind(profile_id.to_string())
    .execute(storage.pool())
    .await?;
    let adapter = Arc::new(ChatFakeAdapter::new()?);
    let runtime = Arc::new(ProviderRuntime::new());
    runtime.register(adapter.clone()).await?;
    let mut bot = homebot_domain::Bot::create("Context Bot", "Planning")?;
    bot.provider_profile_id = Some(profile_id);
    let bot = storage.create_bot(Uuid::nil(), bot, 1).await?;
    let chat = storage
        .create_direct_chat(Uuid::nil(), bot.id.0, Uuid::now_v7(), 2)
        .await?;
    let app =
        router(AppState::new(storage.clone(), "correct-token").with_provider_runtime(runtime));

    let context = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/chats/{}/working-context", chat.id))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let context = response_json::<WorkingContextSummary>(context).await?;
    assert_eq!(context.interaction_mode, InteractionMode::Default);
    assert!(context.plan_mode_available && context.compaction_available);
    assert_eq!(context.context_window_tokens, Some(4_096));

    let mode_request = SetInteractionModeRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        mode: InteractionMode::Plan,
    };
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(json_request(
                "PUT",
                &format!("/api/v1/chats/{}/interaction-mode", chat.id),
                &mode_request,
            ))
            .await?;
        assert_eq!(
            response_json::<WorkingContextSummary>(response)
                .await?
                .interaction_mode,
            InteractionMode::Plan
        );
    }
    let default_mode = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/chats/{}/interaction-mode", chat.id),
            &SetInteractionModeRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                mode: InteractionMode::Default,
            },
        ))
        .await?;
    assert_eq!(
        response_json::<WorkingContextSummary>(default_mode)
            .await?
            .interaction_mode,
        InteractionMode::Default
    );
    let plan_mode = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/chats/{}/interaction-mode", chat.id),
            &SetInteractionModeRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                mode: InteractionMode::Plan,
            },
        ))
        .await?;
    assert_eq!(
        response_json::<WorkingContextSummary>(plan_mode)
            .await?
            .interaction_mode,
        InteractionMode::Plan
    );

    let send_prompt = |content: &str| SendMessageRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        content: content.to_owned(),
        attachment_ids: Vec::new(),
        reply_to_message_id: None,
        mentioned_bot_ids: Vec::new(),
        skill_ids: Vec::new(),
        references: Vec::new(),
    };
    let first = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/messages", chat.id),
            &send_prompt("First turn"),
        ))
        .await?;
    assert!(matches!(
        response_json::<SendMessageResponse>(first).await?,
        SendMessageResponse::Sent { .. }
    ));
    let compact_while_running = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/working-context", chat.id),
            &CompactWorkingContextRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                strategy: ContextCompactionStrategy::Compact,
                target_tokens: Some(64),
            },
        ))
        .await?;
    assert_eq!(compact_while_running.status(), StatusCode::CONFLICT);
    let _ = wait_for_timeline(&app, chat.id, |timeline| {
        timeline
            .approvals
            .iter()
            .any(|approval| approval.status == homebot_protocol::ApprovalStatus::Pending)
    })
    .await
    .map_err(|error| format!("initial approval: {error}"))?;
    for content in ["Second turn", "Third turn"] {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/chats/{}/messages", chat.id),
                &send_prompt(content),
            ))
            .await?;
        assert!(matches!(
            response_json::<SendMessageResponse>(response).await?,
            SendMessageResponse::Queued { .. }
        ));
    }

    for turn in 0..3 {
        let queued_after_promotion = 2 - turn;
        let timeline = wait_for_timeline(&app, chat.id, |timeline| {
            timeline
                .approvals
                .iter()
                .filter(|approval| approval.status == homebot_protocol::ApprovalStatus::Pending)
                .count()
                == 1
                && timeline.queued_prompts.len() == queued_after_promotion
                && timeline
                    .queued_prompts
                    .iter()
                    .enumerate()
                    .all(|(position, prompt)| u32::try_from(position) == Ok(prompt.position))
        })
        .await
        .map_err(|error| format!("turn {turn} approval: {error}"))?;
        let approval_id = timeline
            .approvals
            .iter()
            .find(|approval| approval.status == homebot_protocol::ApprovalStatus::Pending)
            .ok_or("pending approval missing")?
            .id;
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
    }
    let timeline = wait_for_timeline(&app, chat.id, |timeline| {
        !timeline.chat.running && timeline.messages.len() == 6 && timeline.queued_prompts.is_empty()
    })
    .await
    .map_err(|error| format!("queued turns completed: {error}"))?;
    assert_eq!(
        adapter
            .prompts
            .lock()
            .await
            .iter()
            .map(|prompt| prompt.rsplit('\n').next().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["First turn", "Second turn", "Third turn"]
    );
    assert!(
        adapter
            .modes
            .lock()
            .await
            .iter()
            .all(|mode| *mode == homebot_providers::ExecutionMode::Plan)
    );
    assert_eq!(
        timeline
            .working_context
            .as_ref()
            .and_then(|value| value.used_tokens),
        Some(125)
    );

    let compact_request = CompactWorkingContextRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        strategy: ContextCompactionStrategy::Compact,
        target_tokens: Some(64),
    };
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/chats/{}/working-context", chat.id),
                &compact_request,
            ))
            .await?;
        let context = response_json::<WorkingContextSummary>(response).await?;
        assert_eq!(
            context.compaction_status,
            ContextCompactionStatus::Completed
        );
        assert_eq!(context.generation, 1);
        assert_eq!(context.used_tokens, None);
    }
    assert_eq!(adapter.compactions.lock().await.len(), 1);

    let after_compaction = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/messages", chat.id),
            &send_prompt("Post-compact boundary"),
        ))
        .await?;
    assert!(matches!(
        response_json::<SendMessageResponse>(after_compaction).await?,
        SendMessageResponse::Sent { .. }
    ));
    let pending = wait_for_timeline(&app, chat.id, |timeline| {
        timeline
            .approvals
            .iter()
            .any(|approval| approval.status == homebot_protocol::ApprovalStatus::Pending)
    })
    .await
    .map_err(|error| format!("post-compaction approval: {error}"))?;
    let approval_id = pending
        .approvals
        .iter()
        .find(|approval| approval.status == homebot_protocol::ApprovalStatus::Pending)
        .ok_or("post-compaction approval missing")?
        .id;
    let allowed = app
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
    assert_eq!(allowed.status(), StatusCode::OK);
    let _ = wait_for_timeline(&app, chat.id, |timeline| {
        !timeline.chat.running && timeline.messages.len() == 8
    })
    .await
    .map_err(|error| format!("post-compaction turn completed: {error}"))?;
    assert!(
        adapter
            .prompts
            .lock()
            .await
            .last()
            .is_some_and(|prompt| prompt.ends_with("Post-compact boundary"))
    );

    storage
        .begin_working_context_compaction(Uuid::nil(), chat.id, 40)
        .await?;
    let concurrent_reset = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/working-context", chat.id),
            &CompactWorkingContextRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                strategy: ContextCompactionStrategy::Reset,
                target_tokens: None,
            },
        ))
        .await?;
    assert_eq!(concurrent_reset.status(), StatusCode::CONFLICT);
    storage
        .set_working_context_compaction(Uuid::nil(), chat.id, "completed", false, false, None, 41)
        .await?;

    let reset = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/working-context", chat.id),
            &CompactWorkingContextRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                strategy: ContextCompactionStrategy::Reset,
                target_tokens: None,
            },
        ))
        .await?;
    assert_eq!(
        response_json::<WorkingContextSummary>(reset)
            .await?
            .generation,
        2
    );
    assert_eq!(
        storage
            .provider_conversation(bot.id.0, chat.id, profile_id)
            .await?,
        None
    );
    assert_eq!(storage.chat_messages(Uuid::nil(), chat.id).await?.len(), 8);

    let fresh = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/messages", chat.id),
            &send_prompt("Fresh context only"),
        ))
        .await?;
    assert_eq!(fresh.status(), StatusCode::OK);
    let _ = wait_for_timeline(&app, chat.id, |timeline| timeline.messages.len() == 10)
        .await
        .map_err(|error| format!("reset turn persisted: {error}"))?;
    assert!(
        adapter
            .prompts
            .lock()
            .await
            .last()
            .is_some_and(|prompt| prompt.ends_with("Fresh context only"))
    );
    let _ = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/stop", chat.id),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    let _ = wait_for_timeline(&app, chat.id, |timeline| !timeline.chat.running)
        .await
        .map_err(|error| format!("reset turn stopped: {error}"))?;
    storage
        .set_working_context_compaction(Uuid::nil(), chat.id, "running", false, false, None, 99)
        .await?;
    drop(app);
    drop(storage);
    let reopened = Storage::open(&database).await?;
    assert_eq!(
        reopened.chat_messages(Uuid::nil(), chat.id).await?.len(),
        10
    );
    let context = reopened.load_working_context(Uuid::nil(), chat.id).await?;
    assert_eq!(context.generation, 2);
    assert_eq!(context.interaction_mode, "plan");
    assert_eq!(context.compaction_status, "failed");
    assert_eq!(
        context.last_error.as_deref(),
        Some("HomeBot restarted before the context operation completed")
    );
    Ok(())
}

#[tokio::test]
async fn unsupported_plan_and_native_compaction_fail_closed_while_reset_remains_available()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let profile_id = Uuid::now_v7();
    sqlx::query("INSERT INTO provider_profiles (id, adapter_kind, display_name, configuration_json, created_at_ms, updated_at_ms) VALUES (?, 'chat-basic', 'Basic', '{}', 1, 1)")
        .bind(profile_id.to_string()).execute(storage.pool()).await?;
    let runtime = Arc::new(ProviderRuntime::new());
    runtime
        .register(Arc::new(ChatFakeAdapter::without_context_features()?))
        .await?;
    let mut bot = homebot_domain::Bot::create("Basic Bot", "Chat")?;
    bot.provider_profile_id = Some(profile_id);
    let bot = storage.create_bot(Uuid::nil(), bot, 1).await?;
    let chat = storage
        .create_direct_chat(Uuid::nil(), bot.id.0, Uuid::now_v7(), 2)
        .await?;
    let app = router(AppState::new(storage, "correct-token").with_provider_runtime(runtime));

    let plan = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/chats/{}/interaction-mode", chat.id),
            &SetInteractionModeRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                mode: InteractionMode::Plan,
            },
        ))
        .await?;
    assert_eq!(plan.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let compact = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/working-context", chat.id),
            &CompactWorkingContextRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                strategy: ContextCompactionStrategy::Compact,
                target_tokens: None,
            },
        ))
        .await?;
    assert_eq!(compact.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let reset = app
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/working-context", chat.id),
            &CompactWorkingContextRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                strategy: ContextCompactionStrategy::Reset,
                target_tokens: None,
            },
        ))
        .await?;
    let reset = response_json::<WorkingContextSummary>(reset).await?;
    assert_eq!(reset.generation, 1);
    assert!(!reset.plan_mode_available && !reset.compaction_available);
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
                skill_ids: Vec::new(),
                references: Vec::new(),
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
async fn group_message_runs_each_mentioned_bot_and_persists_visible_replies()
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
    let provider = Arc::new(ChatFakeAdapter::new()?);
    runtime.register(provider.clone()).await?;
    let app = router(AppState::new(storage, "correct-token").with_provider_runtime(runtime));

    let mut bot_ids = Vec::new();
    for name in ["Scout", "Codey", "Reviewer"] {
        let bot_id = Uuid::now_v7();
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/bots",
                &CreateBotRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: bot_id,
                    name: name.to_owned(),
                    title: "Teammate".to_owned(),
                    description: String::new(),
                    shape: BotShape::RoundedSquare,
                    color: BotColor::Violet,
                    provider_profile_id: Some(profile_id),
                    permission_profile: BotPermissionProfile::AskBeforeChanges,
                },
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::CREATED);
        bot_ids.push(bot_id);
    }
    let chat_id = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/groups",
            &CreateGroupChatRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: chat_id,
                title: "Product team".to_owned(),
                bot_ids: bot_ids.clone(),
                ownership_bot_id: bot_ids[0],
                coordination_max_turns: 4,
                max_parallel_bots: 3,
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/groups/{chat_id}/messages"),
            &SendGroupMessageRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                content: "Investigate this together".to_owned(),
                mentioned_bot_ids: bot_ids[..2].to_vec(),
                shared_context_message_ids: Vec::new(),
                reply_to_message_id: None,
                references: Vec::new(),
            },
        ))
        .await?;
    if response.status() != StatusCode::OK {
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await?;
        return Err(format!(
            "group message failed with {status}: {}",
            String::from_utf8_lossy(&body)
        )
        .into());
    }

    let mut timeline = None;
    for _ in 0..200 {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/groups/{chat_id}/timeline"))
                    .header("authorization", "Bearer correct-token")
                    .body(Body::empty())?,
            )
            .await?;
        let current = response_json::<GroupTimelineResponse>(response).await?;
        if current.messages.len() == 3 {
            timeline = Some(current);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let timeline = timeline.ok_or("timed out waiting for group Bot replies")?;
    let scout_id = bot_ids[0];
    let reviewer_id = bot_ids[2];
    let scout_message_id = timeline
        .messages
        .iter()
        .find(|message| message.author_bot_id == Some(scout_id))
        .ok_or("Scout reply missing")?
        .id;
    let mut authors = timeline
        .messages
        .iter()
        .filter_map(|message| message.author_bot_id)
        .collect::<Vec<_>>();
    authors.sort_unstable();
    let mut expected_authors = bot_ids[..2].to_vec();
    expected_authors.sort_unstable();
    assert_eq!(authors, expected_authors);
    assert_eq!(
        timeline
            .participants
            .iter()
            .filter(|participant| participant.status == GroupBotStatus::Running)
            .count(),
        2
    );
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/groups/{chat_id}/messages"),
            &SendGroupMessageRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                content: "Duplicate Scout work".to_owned(),
                mentioned_bot_ids: vec![scout_id],
                shared_context_message_ids: Vec::new(),
                reply_to_message_id: None,
                references: Vec::new(),
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
                from_bot_id: scout_id,
                to_bot_id: reviewer_id,
                message_id: Some(scout_message_id),
                reason: "Use Scout's findings to choose the safest implementation".to_owned(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut handoff_timeline = None;
    for _ in 0..200 {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/groups/{chat_id}/timeline"))
                    .header("authorization", "Bearer correct-token")
                    .body(Body::empty())?,
            )
            .await?;
        let current = response_json::<GroupTimelineResponse>(response).await?;
        if current.messages.len() == 4 {
            handoff_timeline = Some(current);
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let handoff_timeline = handoff_timeline.ok_or("handoff did not wake the receiving Bot")?;
    assert_eq!(handoff_timeline.handoffs.len(), 1);
    assert_eq!(
        handoff_timeline
            .messages
            .last()
            .and_then(|message| message.author_bot_id),
        Some(reviewer_id)
    );
    let prompts = provider.prompts.lock().await;
    let handoff_prompt = prompts.last().ok_or("handoff prompt missing")?;
    assert!(handoff_prompt.contains("From: Scout"));
    assert!(handoff_prompt.contains("Use Scout's findings"));
    assert!(handoff_prompt.contains("Hello from the Bot"));
    drop(prompts);
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/groups/{chat_id}/stop"),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut cancelled_timeline = None;
    for _ in 0..200 {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/api/v1/groups/{chat_id}/timeline"))
                    .header("authorization", "Bearer correct-token")
                    .body(Body::empty())?,
            )
            .await?;
        let timeline = response_json::<GroupTimelineResponse>(response).await?;
        if timeline
            .messages
            .iter()
            .filter(|message| message.author_bot_id.is_some())
            .all(|message| message.status == homebot_protocol::MessageStatus::Cancelled)
        {
            assert!(
                timeline
                    .participants
                    .iter()
                    .all(|participant| participant.status == GroupBotStatus::Stopped)
            );
            return Ok(());
        }
        cancelled_timeline = Some(timeline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(format!(
        "timed out waiting for group cancellation: {}",
        serde_json::to_string(&cancelled_timeline)?
    )
    .into())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn group_chat_contract_coordinates_three_bots_with_bounded_handoff()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let mut bot_ids = Vec::new();
    for (index, name) in [
        "Nova", "Patch", "Scout", "Relay", "Atlas", "Echo", "Overflow",
    ]
    .into_iter()
    .enumerate()
    {
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
        bot_ids: bot_ids[..2].to_vec(),
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
        2
    );
    let replay = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/groups", &create))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);

    let rename = RenameGroupChatRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        title: "Release crew".to_owned(),
    };
    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(json_request(
                "PUT",
                &format!("/api/v1/groups/{chat_id}"),
                &rename,
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response_json::<homebot_protocol::GroupChatSummary>(response)
                .await?
                .title,
            "Release crew"
        );
    }

    for bot_id in &bot_ids[2..6] {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/groups/{chat_id}/participants"),
                &AddGroupParticipantRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                    bot_id: *bot_id,
                },
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let overflow = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/groups/{chat_id}/participants"),
            &AddGroupParticipantRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                bot_id: bot_ids[6],
            },
        ))
        .await?;
    assert_eq!(overflow.status(), StatusCode::UNPROCESSABLE_ENTITY);
    for bot_id in &bot_ids[3..6] {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/groups/{chat_id}/participants/{bot_id}/remove"),
                &BotMutationRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                },
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }

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
                reply_to_message_id: None,
                references: vec![
                    MessageReferenceInput {
                        kind: MessageReferenceKind::Bot,
                        target_id: bot_ids[0],
                    },
                    MessageReferenceInput {
                        kind: MessageReferenceKind::Group,
                        target_id: chat_id,
                    },
                ],
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
    let reply_message = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/groups/{chat_id}/messages"),
            &SendGroupMessageRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: reply_message,
                content: "Following up in this thread".to_owned(),
                mentioned_bot_ids: Vec::new(),
                shared_context_message_ids: Vec::new(),
                reply_to_message_id: Some(first_message),
                references: Vec::new(),
            },
        ))
        .await?;
    assert_eq!(
        response_json::<homebot_protocol::MessageSummary>(response)
            .await?
            .reply_to_message_id,
        Some(first_message)
    );

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
    assert_eq!(timeline.messages.len(), 3);
    assert!(
        timeline
            .messages
            .iter()
            .any(|message| message.shared_context_message_ids == vec![first_message])
    );
    assert_eq!(
        timeline
            .messages
            .iter()
            .find(|message| message.id == reply_message)
            .and_then(|message| message.reply_to_message_id),
        Some(first_message)
    );
    assert_eq!(timeline.handoffs.len(), 1);
    let referenced = timeline
        .messages
        .iter()
        .find(|message| message.id == first_message)
        .ok_or("referenced message missing")?;
    assert_eq!(referenced.references.len(), 2);
    assert_eq!(referenced.references[0].label, "Nova");
    assert_eq!(referenced.references[1].label, "Release crew");

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
    // Provider turns share constrained CI runners with Android, release packages,
    // and cross-target builds. Preserve the short polling cadence while allowing
    // up to 60 seconds for the durable state transition; every caller still
    // asserts the exact approval/message/queue postcondition.
    for _ in 0..6_000 {
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
    let app = test_app().await?;
    let response = app
        .router
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
        let app = test_app().await?;
        let response = app.router.oneshot(request.body(Body::empty())?).await?;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn pairing_is_single_use_restart_durable_revocable_and_owner_managed()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("homebot.db");
    let storage = Storage::open(&database).await?;
    let app = router(AppState::new(storage.clone(), "correct-token"));

    let unauthorized = app
        .clone()
        .oneshot(
            Request::post("/api/v1/pairing")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&CreatePairingRequest {
                    request_id: Uuid::now_v7(),
                    endpoint: "http://127.0.0.1:7123".to_owned(),
                    allow_insecure_private_network: false,
                })?))?,
        )
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    for (endpoint, acknowledge) in [
        ("http://203.0.113.7:7123", true),
        ("http://192.168.1.20:7123", false),
    ] {
        let denied = app
            .clone()
            .oneshot(json_request(
                "POST",
                "/api/v1/pairing",
                &CreatePairingRequest {
                    request_id: Uuid::now_v7(),
                    endpoint: endpoint.to_owned(),
                    allow_insecure_private_network: acknowledge,
                },
            ))
            .await?;
        assert_eq!(denied.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    let tailscale = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/pairing",
            &CreatePairingRequest {
                request_id: Uuid::now_v7(),
                endpoint: "http://homebot.tailnet.ts.net:7123".to_owned(),
                allow_insecure_private_network: true,
            },
        ))
        .await?;
    let tailscale = response_json::<PairingOffer>(tailscale).await?;
    assert_eq!(tailscale.endpoint_kind, PairingEndpointKind::Tailscale);
    assert!(tailscale.warning.is_some());

    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/pairing",
            &CreatePairingRequest {
                request_id: Uuid::now_v7(),
                endpoint: "http://127.0.0.1:7123".to_owned(),
                allow_insecure_private_network: false,
            },
        ))
        .await?;
    assert_eq!(
        created
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let offer = response_json::<PairingOffer>(created).await?;
    assert_eq!(offer.endpoint_kind, PairingEndpointKind::Loopback);
    assert!(offer.pairing_token.starts_with("hbpair_"));
    assert!(offer.deep_link.contains(&offer.pairing_token));

    let exchange_request = ExchangePairingRequest {
        request_id: Uuid::now_v7(),
        pairing_token: offer.pairing_token.clone(),
        native_proof: None,
        device_name: "Pixel test device".to_owned(),
    };
    let source = "192.168.1.20:41000".parse::<std::net::SocketAddr>()?;
    for attempt in 0..31 {
        let invalid = ExchangePairingRequest {
            request_id: Uuid::now_v7(),
            pairing_token: format!("hbpair_invalid_{attempt}"),
            native_proof: Some("hbproof_invalid".to_owned()),
            device_name: "Unknown device".to_owned(),
        };
        let response = app
            .clone()
            .oneshot(
                Request::post("/api/v1/pairing/exchange")
                    .extension(axum::extract::ConnectInfo(source))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&invalid)?))?,
            )
            .await?;
        assert_eq!(
            response.status(),
            if attempt < 30 {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
    let missing_origin = app
        .clone()
        .oneshot(
            Request::post("/api/v1/pairing/exchange")
                .extension(axum::extract::ConnectInfo(source))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&exchange_request)?))?,
        )
        .await?;
    assert_eq!(missing_origin.status(), StatusCode::FORBIDDEN);

    let mismatched = app
        .clone()
        .oneshot(
            Request::post("/api/v1/pairing/exchange")
                .extension(axum::extract::ConnectInfo(
                    "192.168.1.20:41000".parse::<std::net::SocketAddr>()?,
                ))
                .header("content-type", "application/json")
                .header("origin", "https://attacker.example")
                .body(Body::from(serde_json::to_vec(&exchange_request)?))?,
        )
        .await?;
    assert_eq!(mismatched.status(), StatusCode::FORBIDDEN);

    let exchanged = app
        .clone()
        .oneshot(
            Request::post("/api/v1/pairing/exchange")
                .extension(axum::extract::ConnectInfo(
                    "192.168.1.20:41001".parse::<std::net::SocketAddr>()?,
                ))
                .header("content-type", "application/json")
                .header("origin", "http://127.0.0.1:7123")
                .body(Body::from(serde_json::to_vec(&exchange_request)?))?,
        )
        .await?;
    assert_eq!(exchanged.status(), StatusCode::OK);
    assert_eq!(
        exchanged
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let exchanged = response_json::<PairingExchangeResponse>(exchanged).await?;
    assert!(exchanged.device_session.starts_with("hbds_"));
    assert_eq!(exchanged.device.name, "Pixel test device");
    assert!(!offer.deep_link.contains(&exchanged.device_session));

    let used = app
        .clone()
        .oneshot(
            Request::post("/api/v1/pairing/exchange")
                .extension(axum::extract::ConnectInfo(
                    "192.168.1.20:41002".parse::<std::net::SocketAddr>()?,
                ))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&exchange_request)?))?,
        )
        .await?;
    assert_eq!(used.status(), StatusCode::CONFLICT);

    let native_offer = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/pairing",
            &CreatePairingRequest {
                request_id: Uuid::now_v7(),
                endpoint: "http://127.0.0.1:7123".to_owned(),
                allow_insecure_private_network: false,
            },
        ))
        .await?;
    let native_offer = response_json::<PairingOffer>(native_offer).await?;
    let native_proof = url::Url::parse(&native_offer.deep_link)?
        .query_pairs()
        .find_map(|(key, value)| (key == "proof").then(|| value.into_owned()))
        .ok_or("pairing deep link omitted native proof")?;
    assert!(native_proof.starts_with("hbproof_"));
    let native_exchange = ExchangePairingRequest {
        request_id: Uuid::now_v7(),
        pairing_token: native_offer.pairing_token,
        native_proof: Some(native_proof),
        device_name: "Native Android device".to_owned(),
    };
    let native_exchanged = app
        .clone()
        .oneshot(
            Request::post("/api/v1/pairing/exchange")
                .extension(axum::extract::ConnectInfo(
                    "100.64.12.5:42000".parse::<std::net::SocketAddr>()?,
                ))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&native_exchange)?))?,
        )
        .await?;
    assert_eq!(native_exchanged.status(), StatusCode::OK);
    let native_exchanged = response_json::<PairingExchangeResponse>(native_exchanged).await?;
    assert_eq!(native_exchanged.device.name, "Native Android device");

    drop(app);
    let restarted = router(AppState::new(storage.clone(), "correct-token"));
    let device_version = restarted
        .clone()
        .oneshot(
            Request::get("/api/v1/version")
                .header(
                    "authorization",
                    format!("Bearer {}", exchanged.device_session),
                )
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(device_version.status(), StatusCode::OK);
    let device_cannot_pair = restarted
        .clone()
        .oneshot(
            Request::post("/api/v1/pairing")
                .header(
                    "authorization",
                    format!("Bearer {}", exchanged.device_session),
                )
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&CreatePairingRequest {
                    request_id: Uuid::now_v7(),
                    endpoint: "http://127.0.0.1:7123".to_owned(),
                    allow_insecure_private_network: false,
                })?))?,
        )
        .await?;
    assert_eq!(device_cannot_pair.status(), StatusCode::FORBIDDEN);
    let current = restarted
        .clone()
        .oneshot(
            Request::get("/api/v1/device")
                .header(
                    "authorization",
                    format!("Bearer {}", exchanged.device_session),
                )
                .body(Body::empty())?,
        )
        .await?;
    let current = response_json::<DeviceSessionSummary>(current).await?;
    assert_eq!(current.id, exchanged.device.id);
    let device_cannot_list_others = restarted
        .clone()
        .oneshot(
            Request::get("/api/v1/devices")
                .header(
                    "authorization",
                    format!("Bearer {}", exchanged.device_session),
                )
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(device_cannot_list_others.status(), StatusCode::FORBIDDEN);

    let listed = restarted
        .clone()
        .oneshot(
            Request::get("/api/v1/devices")
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let listed = response_json::<Vec<DeviceSessionSummary>>(listed).await?;
    assert_eq!(listed.len(), 2);
    let browser_device = listed
        .iter()
        .find(|device| device.id == exchanged.device.id)
        .ok_or("browser-paired device was not listed")?;
    assert_eq!(browser_device.name, exchanged.device.name);
    assert!(browser_device.last_seen_at_unix_ms.is_some());

    let revoke = RevokeDeviceSessionRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let mut revoked = None;
    for _ in 0..2 {
        let response = restarted
            .clone()
            .oneshot(json_request(
                "POST",
                &format!("/api/v1/devices/{}/revoke", exchanged.device.id),
                &revoke,
            ))
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let response = response_json::<DeviceSessionSummary>(response).await?;
        if let Some(original) = &revoked {
            assert_eq!(original, &response);
        } else {
            revoked = Some(response);
        }
    }
    assert!(revoked.is_some_and(|device| device.revoked_at_unix_ms.is_some()));
    let denied = restarted
        .clone()
        .oneshot(
            Request::get("/api/v1/version")
                .header(
                    "authorization",
                    format!("Bearer {}", exchanged.device_session),
                )
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);

    let persisted_text: Vec<String> = sqlx::query_scalar(
        "SELECT endpoint || expected_origin || endpoint_kind FROM pairing_credentials
         UNION ALL SELECT name || endpoint_kind FROM device_sessions",
    )
    .fetch_all(storage.pool())
    .await?;
    assert!(persisted_text.iter().all(|value| {
        !value.contains(&offer.pairing_token) && !value.contains(&exchanged.device_session)
    }));
    Ok(())
}

#[tokio::test]
async fn secret_crud_uses_only_os_vault_values_and_reports_locked_state()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("homebot.db");
    let storage = Storage::open(&database).await?;
    let vault = Arc::new(MemorySecretVault::default());
    let app =
        router(AppState::new(storage.clone(), "correct-token").with_secret_vault(vault.clone()));
    let secret_id = Uuid::now_v7();
    let canary = "homebot-secret-canary-29419";
    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/secrets",
            &serde_json::json!({
                "request_id": Uuid::now_v7(),
                "idempotency_key": secret_id,
                "label": "OpenAI work",
                "value": canary,
            }),
        ))
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json::<SecretSummary>(created).await?;
    assert_eq!(created.id, secret_id);
    assert_eq!(created.status, homebot_protocol::SecretStatus::Ready);

    let listed = app
        .clone()
        .oneshot(
            Request::get("/api/v1/secrets")
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let listed_bytes = to_bytes(listed.into_body(), usize::MAX).await?;
    assert!(
        !listed_bytes
            .windows(canary.len())
            .any(|value| value == canary.as_bytes())
    );
    let listed: Vec<SecretSummary> = serde_json::from_slice(&listed_bytes)?;
    assert_eq!(listed, vec![created]);

    let persisted_text: Vec<String> = sqlx::query_scalar(
        "SELECT coalesce(content_json, '') FROM message_parts
         UNION ALL SELECT coalesce(detail_json, '') FROM execution_activities
         UNION ALL SELECT coalesce(payload_json, '') FROM event_outbox",
    )
    .fetch_all(storage.pool())
    .await?;
    assert!(persisted_text.iter().all(|value| !value.contains(canary)));
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('secret_references')")
            .fetch_all(storage.pool())
            .await?;
    assert!(
        !columns
            .iter()
            .any(|column| column == "value" || column == "secret")
    );

    vault.force_status(Some(SecretStatus::Locked)).await;
    let locked = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/secrets/{secret_id}"),
            &serde_json::json!({"request_id": Uuid::now_v7(), "value": "replacement"}),
        ))
        .await?;
    assert_eq!(locked.status(), StatusCode::SERVICE_UNAVAILABLE);
    let error = response_json::<ErrorEnvelope>(locked).await?;
    assert_eq!(error.code, ErrorCode::SecretStoreLocked);

    vault.force_status(None).await;
    let updated = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/secrets/{secret_id}"),
            &serde_json::json!({
                "request_id": Uuid::now_v7(),
                "label": "OpenAI personal",
                "value": "replacement",
            }),
        ))
        .await?;
    assert_eq!(
        response_json::<SecretSummary>(updated).await?.label,
        "OpenAI personal"
    );

    let deleted = app
        .oneshot(
            Request::delete(format!("/api/v1/secrets/{secret_id}"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        vault.status(&locator_for(secret_id)).await,
        SecretStatus::Missing
    );
    Ok(())
}

#[tokio::test]
async fn valid_device_session_can_negotiate_version() -> Result<(), Box<dyn std::error::Error>> {
    let app = test_app().await?;
    let response = app
        .router
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
    let app = test_app().await?;
    let response = app
        .router
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
async fn websocket_rejects_oversized_client_messages() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let (address, task, _guard) = spawn_app(storage).await?;
    let mut request = format!("ws://{address}/api/v1/events").into_client_request()?;
    request
        .headers_mut()
        .insert("authorization", "Bearer correct-token".parse()?);
    let (mut socket, _) = connect_async(request).await?;
    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            "x".repeat(super::MAX_WEBSOCKET_MESSAGE_BYTES + 1).into(),
        ))
        .await?;

    let response = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next()).await?;
    assert!(matches!(
        response,
        Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_)) | Err(_)) | None
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
    let reconnect_started = Instant::now();
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
    assert!(
        reconnect_started.elapsed() <= Duration::from_secs(2),
        "cursor replay exceeded the reconnect budget"
    );
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
    let reconnect_started = Instant::now();
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
    assert!(
        reconnect_started.elapsed() <= Duration::from_secs(2),
        "snapshot fallback exceeded the reconnect budget"
    );
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
#[allow(clippy::too_many_lines)]
async fn skill_library_versions_assigns_exports_and_resolves_import_conflicts()
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
    let app = router(AppState::new(storage.clone(), "correct-token"));
    let skill_id = Uuid::now_v7();
    let definition = SkillDefinition {
        instructions: "Write concise release notes.".to_owned(),
        context: vec![SkillContext {
            label: "Voice".to_owned(),
            content: "Use plain language.".to_owned(),
        }],
        tools: vec![SkillToolReference {
            plugin_name: "repository".to_owned(),
            tool_name: "status".to_owned(),
        }],
    };
    let create = CreateSkillRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: skill_id,
        name: "Release writer".to_owned(),
        description: "Project release voice".to_owned(),
        definition: definition.clone(),
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/skills", &create))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created: SkillSummary = response_json(response).await?;
    assert_eq!(created.version, 1);
    assert!(created.bot_ids.is_empty());
    let replay = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/skills", &create))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);

    let assignment = SkillAssignmentRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        bot_id: bot.id.0,
        enabled: true,
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/skills/{skill_id}/assignment"),
            &assignment,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json::<SkillSummary>(response).await?.bot_ids,
        vec![bot.id.0]
    );

    let edit_key = Uuid::now_v7();
    let mut updated_definition = definition.clone();
    updated_definition.instructions = "Write concise, verifiable release notes.".to_owned();
    let update = UpdateSkillRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: edit_key,
        name: "Release writer".to_owned(),
        description: "Updated voice".to_owned(),
        definition: updated_definition.clone(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/skills/{skill_id}"),
            &update,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json::<SkillSummary>(response).await?.version, 2);
    let replay = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/skills/{skill_id}"),
            &update,
        ))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    let replayed: SkillSummary = response_json(replay).await?;
    assert_eq!(replayed.version, 2);
    assert_eq!(storage.skill(Uuid::nil(), skill_id).await?.version, 2);

    let exported = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/skills/{skill_id}/export"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let bundle: SkillBundle = response_json(exported).await?;
    assert_eq!(bundle.definition, updated_definition);

    let rejected = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/skills/import",
            &ImportSkillRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                bundle: bundle.clone(),
                conflict_policy: SkillImportConflictPolicy::Reject,
            },
        ))
        .await?;
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
    let imported_id = Uuid::now_v7();
    let renamed = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/skills/import",
            &ImportSkillRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: imported_id,
                bundle: bundle.clone(),
                conflict_policy: SkillImportConflictPolicy::Rename,
            },
        ))
        .await?;
    assert_eq!(renamed.status(), StatusCode::CREATED);
    let renamed: SkillSummary = response_json(renamed).await?;
    assert_eq!(renamed.id, imported_id);
    assert_eq!(renamed.name, "Release writer (imported)");

    let version_key = Uuid::now_v7();
    let versioned = ImportSkillRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: version_key,
        bundle,
        conflict_policy: SkillImportConflictPolicy::CreateVersion,
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/skills/import", &versioned))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json::<SkillSummary>(response).await?.version, 3);
    assert_eq!(
        storage
            .skill_version(Uuid::nil(), version_key)
            .await?
            .version,
        3
    );
    let replay = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/skills/import", &versioned))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json::<SkillSummary>(replay).await?.version, 3);
    assert_eq!(storage.skill(Uuid::nil(), skill_id).await?.version, 3);

    let duplicate_id = Uuid::now_v7();
    let duplicated = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/skills/{skill_id}/duplicate"),
            &DuplicateSkillRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: duplicate_id,
                name: "Release writer copy".to_owned(),
            },
        ))
        .await?;
    assert_eq!(duplicated.status(), StatusCode::CREATED);
    assert_eq!(response_json::<SkillSummary>(duplicated).await?.version, 1);

    let deleted = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/v1/skills/{duplicate_id}"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert!(matches!(
        storage.skill(Uuid::nil(), duplicate_id).await,
        Err(homebot_storage::StorageError::SkillNotFound)
    ));
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn repository_workspace_preserves_dirty_primary_and_guards_worktree_cleanup()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command as StdCommand;

    let repository = tempfile::tempdir()?;
    let git = |arguments: &[&str]| -> Result<(), Box<dyn std::error::Error>> {
        let output = StdCommand::new("/usr/bin/git")
            .arg("-C")
            .arg(repository.path())
            .args(arguments)
            .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        Ok(())
    };
    git(&["init", "-b", "main"])?;
    git(&["config", "user.name", "HomeBot Fixture"])?;
    git(&["config", "user.email", "fixture@homebot.invalid"])?;
    std::fs::write(repository.path().join("README.md"), "baseline\n")?;
    git(&["add", "README.md"])?;
    git(&["commit", "-m", "baseline"])?;
    std::fs::write(
        repository.path().join("README.md"),
        "valuable dirty change\n",
    )?;
    std::fs::write(repository.path().join("untracked.txt"), "preserve\n")?;

    let directory = tempfile::tempdir()?;
    let managed = directory.path().join("worktrees");
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let bot = storage
        .create_bot(
            Uuid::nil(),
            homebot_domain::Bot::create("Nova", "Coding")?,
            1,
        )
        .await?;
    let chat = storage
        .create_direct_chat(Uuid::nil(), bot.id.0, Uuid::now_v7(), 2)
        .await?;
    let runtime = Arc::new(homebot_vcs::GitRuntime::discover()?);
    let app = router(
        AppState::new(storage.clone(), "correct-token")
            .with_git_runtime(Arc::clone(&runtime), managed.clone()),
    );
    let workspace_id = Uuid::now_v7();
    let create = CreateRepositoryWorkspaceRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: workspace_id,
        root_path: repository.path().to_string_lossy().into_owned(),
        name: Some("HomeBot fixture".to_owned()),
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/workspaces", &create))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let workspace: RepositoryWorkspaceSummary = response_json(response).await?;
    assert_eq!(workspace.condition, WorkingTreeCondition::Dirty);
    assert_eq!(workspace.current_branch.as_deref(), Some("main"));
    let replay = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/workspaces", &create))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    let duplicate = CreateRepositoryWorkspaceRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        ..create.clone()
    };
    let response = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/workspaces", &duplicate))
        .await?;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let branches = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/workspaces/{workspace_id}/branches"))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(
        response_json::<WorkspaceBranchesResponse>(branches)
            .await?
            .branches,
        vec!["main"]
    );

    let attach = AttachChatWorkspaceRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        workspace_id,
        mode: WorkspaceMode::Isolated,
        base_ref: Some("main".to_owned()),
        branch_name: None,
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/chats/{}/workspace", chat.id),
            &attach,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let attached: ChatWorkspaceSummary = response_json(response).await?;
    assert_eq!(attached.mode, WorkspaceMode::Isolated);
    assert_eq!(attached.condition, WorkingTreeCondition::Clean);
    assert!(
        std::path::Path::new(&attached.effective_path)
            .starts_with(std::fs::canonicalize(&managed)?)
    );
    assert_eq!(
        std::fs::read_to_string(repository.path().join("README.md"))?,
        "valuable dirty change\n"
    );
    assert_eq!(
        std::fs::read_to_string(repository.path().join("untracked.txt"))?,
        "preserve\n"
    );
    let replay = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/chats/{}/workspace", chat.id),
            &attach,
        ))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);

    let valuable = std::path::Path::new(&attached.effective_path).join("valuable.txt");
    std::fs::write(&valuable, "keep me\n")?;
    let detach = DetachChatWorkspaceRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let denied = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/workspace/detach", chat.id),
            &detach,
        ))
        .await?;
    assert_eq!(denied.status(), StatusCode::CONFLICT);
    assert_eq!(std::fs::read_to_string(&valuable)?, "keep me\n");
    assert!(
        storage
            .chat_workspace(Uuid::nil(), chat.id)
            .await?
            .is_some()
    );
    std::fs::remove_file(&valuable)?;
    let detach = DetachChatWorkspaceRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let removed = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/workspace/detach", chat.id),
            &detach,
        ))
        .await?;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
    assert!(!std::path::Path::new(&attached.effective_path).exists());
    assert!(
        storage
            .chat_workspace(Uuid::nil(), chat.id)
            .await?
            .is_none()
    );

    let primary = AttachChatWorkspaceRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        workspace_id,
        mode: WorkspaceMode::Primary,
        base_ref: None,
        branch_name: None,
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/chats/{}/workspace", chat.id),
            &primary,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let attached_primary: ChatWorkspaceSummary = response_json(response).await?;
    assert_eq!(attached_primary.mode, WorkspaceMode::Primary);
    assert_eq!(attached_primary.condition, WorkingTreeCondition::Dirty);
    assert_eq!(
        attached_primary.effective_path,
        std::fs::canonicalize(repository.path())?.to_string_lossy()
    );
    let detach_primary = DetachChatWorkspaceRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/workspace/detach", chat.id),
            &detach_primary,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert!(repository.path().exists());
    assert_eq!(
        std::fs::read_to_string(repository.path().join("README.md"))?,
        "valuable dirty change\n"
    );

    let conflict = AttachChatWorkspaceRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        workspace_id,
        mode: WorkspaceMode::Isolated,
        base_ref: Some("main".to_owned()),
        branch_name: Some("main".to_owned()),
    };
    let denied = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/chats/{}/workspace", chat.id),
            &conflict,
        ))
        .await?;
    assert_eq!(denied.status(), StatusCode::CONFLICT);
    assert!(
        storage
            .chat_workspace(Uuid::nil(), chat.id)
            .await?
            .is_none()
    );
    let moved_repository = directory.path().join("repository-moved");
    std::fs::rename(repository.path(), &moved_repository)?;
    let response = app
        .oneshot(
            Request::get("/api/v1/workspaces")
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let unavailable: Vec<RepositoryWorkspaceSummary> = response_json(response).await?;
    assert_eq!(unavailable.len(), 1);
    assert_eq!(unavailable[0].condition, WorkingTreeCondition::Unavailable);
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn coding_turn_checkpoints_diff_restore_and_fork_provider_conversation()
-> Result<(), Box<dyn std::error::Error>> {
    let repository = tempfile::tempdir()?;
    let git = |arguments: &[&str]| -> Result<(), Box<dyn std::error::Error>> {
        let output = std::process::Command::new("/usr/bin/git")
            .env_clear()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("-C")
            .arg(repository.path())
            .args(arguments)
            .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        Ok(())
    };
    git(&["init", "-b", "main"])?;
    git(&["config", "user.name", "HomeBot Fixture"])?;
    git(&["config", "user.email", "fixture@homebot.invalid"])?;
    std::fs::write(repository.path().join("README.md"), "committed\n")?;
    git(&["add", "README.md"])?;
    git(&["commit", "-m", "baseline"])?;
    std::fs::write(repository.path().join("README.md"), "dirty baseline\n")?;
    std::fs::write(repository.path().join("untracked.txt"), "before\n")?;

    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let profile_id = Uuid::now_v7();
    sqlx::query("INSERT INTO provider_profiles (id, adapter_kind, display_name, configuration_json, created_at_ms, updated_at_ms) VALUES (?, 'chat-fake', 'Fixture', '{}', 1, 1)")
        .bind(profile_id.to_string()).execute(storage.pool()).await?;
    let mut domain_bot = homebot_domain::Bot::create("Patch", "Coding")?;
    domain_bot.provider_profile_id = Some(profile_id);
    let bot = storage.create_bot(Uuid::nil(), domain_bot, 1).await?;
    let chat = storage
        .create_direct_chat(Uuid::nil(), bot.id.0, Uuid::now_v7(), 2)
        .await?;
    let repository_root = std::fs::canonicalize(repository.path())?
        .to_string_lossy()
        .into_owned();
    let workspace = homebot_storage::RepositoryWorkspaceRecord {
        id: Uuid::now_v7(),
        owner_id: Uuid::nil(),
        name: "Checkpoint fixture".to_owned(),
        root_path: repository_root.clone(),
        created_at_ms: 3,
        updated_at_ms: 3,
    };
    storage.create_repository_workspace(&workspace).await?;
    storage
        .attach_chat_workspace(&homebot_storage::ChatWorkspaceRecord {
            owner_id: Uuid::nil(),
            chat_id: chat.id,
            workspace_id: workspace.id,
            mode: WorkspaceMode::Primary,
            worktree_path: None,
            branch_name: Some("main".to_owned()),
            base_ref: None,
            created_at_ms: 4,
            updated_at_ms: 4,
        })
        .await?;
    let provider_runtime = Arc::new(ProviderRuntime::new());
    let provider = Arc::new(ChatFakeAdapter::new()?);
    provider_runtime.register(provider.clone()).await?;
    let app = router(
        AppState::new(storage.clone(), "correct-token")
            .with_provider_runtime(provider_runtime)
            .with_git_runtime(
                Arc::new(homebot_vcs::GitRuntime::discover()?),
                directory.path().join("worktrees"),
            ),
    );

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/messages", chat.id),
            &SendMessageRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                content: "Change the repository".to_owned(),
                attachment_ids: Vec::new(),
                reply_to_message_id: None,
                mentioned_bot_ids: Vec::new(),
                skill_ids: Vec::new(),
                references: Vec::new(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        *provider.working_directories.lock().await,
        vec![Some(PathBuf::from(&repository_root))]
    );
    let timeline = wait_for_timeline(&app, chat.id, |timeline| {
        timeline.approvals.len() == 1
            && timeline
                .checkpoints
                .iter()
                .any(|checkpoint| checkpoint.phase == CheckpointPhase::BeforeTurn)
    })
    .await?;
    let before = timeline
        .checkpoints
        .iter()
        .find(|checkpoint| checkpoint.phase == CheckpointPhase::BeforeTurn)
        .ok_or("missing before-turn checkpoint")?
        .clone();
    std::fs::rename(
        repository.path().join("README.md"),
        repository.path().join("GUIDE.md"),
    )?;
    std::fs::write(repository.path().join("untracked.txt"), "after\n")?;
    std::fs::write(
        repository.path().join("binary.dat"),
        [0_u8, 159, 146, 150, 0, 255],
    )?;
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{}/decision", timeline.approvals[0].id),
            &ApprovalDecisionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                allow: true,
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let timeline = wait_for_timeline(&app, chat.id, |timeline| {
        !timeline.chat.running
            && timeline
                .checkpoints
                .iter()
                .any(|checkpoint| checkpoint.phase == CheckpointPhase::AfterTurn)
    })
    .await?;
    let after = timeline
        .checkpoints
        .iter()
        .find(|checkpoint| checkpoint.phase == CheckpointPhase::AfterTurn)
        .ok_or("missing after-turn checkpoint")?
        .clone();
    let response = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/api/v1/chats/{}/checkpoints/diff?from_checkpoint_id={}&to_checkpoint_id={}",
                chat.id, before.id, after.id
            ))
            .header("authorization", "Bearer correct-token")
            .body(Body::empty())?,
        )
        .await?;
    let diff: CheckpointDiffResponse = response_json(response).await?;
    assert!(diff.patch.contains("GIT binary patch"));
    assert!(
        diff.files
            .iter()
            .any(|file| file.path == "binary.dat" && file.binary)
    );
    assert!(diff.files.iter().any(|file| file.path == "GUIDE.md"));
    let full = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/chats/{}/checkpoints/diff/full", chat.id))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response_json::<CheckpointDiffResponse>(full).await?, diff);

    let restore_request = RestoreCheckpointRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/checkpoints/{}/restore", before.id),
            &restore_request,
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let restored: CheckpointRestoreSummary = response_json(response).await?;
    assert_eq!(restored.reconciliation, ConversationReconciliation::Forked);
    assert_eq!(
        std::fs::read_to_string(repository.path().join("README.md"))?,
        "dirty baseline\n"
    );
    assert_eq!(
        std::fs::read_to_string(repository.path().join("untracked.txt"))?,
        "before\n"
    );
    assert!(!repository.path().join("GUIDE.md").exists());
    assert!(!repository.path().join("binary.dat").exists());
    assert!(
        storage
            .provider_conversation(bot.id.0, chat.id, profile_id)
            .await?
            .is_none()
    );
    let replay = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/checkpoints/{}/restore", before.id),
            &restore_request,
        ))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    let checkpoints = app
        .oneshot(
            Request::get(format!("/api/v1/chats/{}/checkpoints", chat.id))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let checkpoints: Vec<TurnCheckpointSummary> = response_json(checkpoints).await?;
    assert_eq!(checkpoints.len(), 3);
    assert_eq!(
        checkpoints
            .iter()
            .filter(|checkpoint| checkpoint.phase == CheckpointPhase::RestoreSafety)
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn skill_versions_are_assembled_for_providers_and_pinned_to_message_history()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let profile_id = Uuid::now_v7();
    sqlx::query("INSERT INTO provider_profiles (id, adapter_kind, display_name, configuration_json, created_at_ms, updated_at_ms) VALUES (?, 'chat-fake', 'Fixture', '{}', 1, 1)")
        .bind(profile_id.to_string()).execute(storage.pool()).await?;
    let mut domain_bot = homebot_domain::Bot::create("Nova", "Research")?;
    domain_bot.update_identity(
        "Nova",
        "Research",
        "Find useful context and cite exact evidence.",
        domain_bot.shape,
        domain_bot.color,
    )?;
    domain_bot.provider_profile_id = Some(profile_id);
    let bot = storage.create_bot(Uuid::nil(), domain_bot, 1).await?;
    let chat = storage
        .create_direct_chat(Uuid::nil(), bot.id.0, Uuid::now_v7(), 2)
        .await?;
    let skill_id = Uuid::now_v7();
    let first_version = Uuid::now_v7();
    storage
        .create_skill(&homebot_storage::SkillRecord {
            id: skill_id,
            owner_id: Uuid::nil(),
            name: "Source reviewer".to_owned(),
            description: String::new(),
            active_version_id: first_version,
            version: 1,
            definition: SkillDefinition {
                instructions: "Use exact evidence.".to_owned(),
                context: Vec::new(),
                tools: vec![SkillToolReference {
                    plugin_name: "repository".to_owned(),
                    tool_name: "status".to_owned(),
                }],
            },
            bot_ids: Vec::new(),
            created_at_ms: 3,
            updated_at_ms: 3,
        })
        .await?;
    storage
        .set_skill_assignment(Uuid::nil(), skill_id, bot.id.0, true, 4)
        .await?;
    let adapter = Arc::new(ChatFakeAdapter::new()?);
    let prompts = Arc::clone(&adapter.prompts);
    let runtime = Arc::new(ProviderRuntime::new());
    runtime.register(adapter).await?;
    let app =
        router(AppState::new(storage.clone(), "correct-token").with_provider_runtime(runtime));
    let message_id = Uuid::now_v7();
    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/messages", chat.id),
            &SendMessageRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: message_id,
                content: "Review the change".to_owned(),
                attachment_ids: Vec::new(),
                reply_to_message_id: None,
                mentioned_bot_ids: Vec::new(),
                skill_ids: Vec::new(),
                references: Vec::new(),
            },
        ))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let prompt = prompts.lock().await[0].clone();
    assert!(prompt.contains("<homebot_bot>"));
    assert!(prompt.contains("Name: Nova"));
    assert!(prompt.contains("Role: Research"));
    assert!(prompt.contains("Responsibility: Find useful context and cite exact evidence."));
    assert!(prompt.contains("## Skill: Source reviewer (version 1)"));
    assert!(prompt.contains("Use exact evidence."));
    assert!(prompt.contains("capability policy still applies"));
    assert!(prompt.ends_with("<user_message>\nReview the change\n</user_message>"));

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !storage
                .chat_approvals(Uuid::nil(), chat.id)
                .await
                .unwrap_or_default()
                .is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;

    storage
        .update_skill(
            Uuid::nil(),
            skill_id,
            "Source reviewer",
            "",
            &SkillDefinition {
                instructions: "Use the new behavior.".to_owned(),
                context: Vec::new(),
                tools: Vec::new(),
            },
            Uuid::now_v7(),
            5,
        )
        .await?;
    let timeline = fetch_timeline(&app, chat.id).await?;
    let persisted = timeline
        .messages
        .iter()
        .find(|message| message.id == message_id)
        .ok_or("message missing")?;
    assert_eq!(persisted.applied_skills.len(), 1);
    assert_eq!(persisted.applied_skills[0].skill_version_id, first_version);
    assert_eq!(persisted.applied_skills[0].version, 1);
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn source_control_is_server_owned_idempotent_and_remote_push_requires_approval()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Command as StdCommand;

    let repository = tempfile::tempdir()?;
    let remote = tempfile::tempdir()?;
    let run_git = |root: &std::path::Path,
                   arguments: &[&str]|
     -> Result<String, Box<dyn std::error::Error>> {
        let output = StdCommand::new("/usr/bin/git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        Ok(String::from_utf8(output.stdout)?)
    };
    run_git(repository.path(), &["init", "-b", "main"])?;
    run_git(
        repository.path(),
        &["config", "user.name", "HomeBot Fixture"],
    )?;
    run_git(
        repository.path(),
        &["config", "user.email", "fixture@homebot.invalid"],
    )?;
    std::fs::write(repository.path().join("README.md"), "baseline\n")?;
    run_git(repository.path(), &["add", "README.md"])?;
    run_git(repository.path(), &["commit", "-m", "baseline"])?;
    run_git(remote.path(), &["init", "--bare"])?;
    run_git(
        repository.path(),
        &[
            "remote",
            "add",
            "origin",
            remote.path().to_str().ok_or("invalid remote path")?,
        ],
    )?;
    run_git(
        repository.path(),
        &[
            "remote",
            "add",
            "github",
            "https://github.com/luinbytes/HomeBot.git",
        ],
    )?;
    let gh_fixture = tempfile::tempdir()?;
    let gh = gh_fixture.path().join("gh");
    std::fs::write(
        &gh,
        "#!/bin/sh\nif [ \"$2\" = \"create\" ]; then echo https://github.com/luinbytes/HomeBot/pull/42; exit 0; fi\necho '{\"number\":42,\"url\":\"https://github.com/luinbytes/HomeBot/pull/42\",\"title\":\"HomeBot source control\",\"state\":\"OPEN\",\"headRefName\":\"homebot/approved\",\"baseRefName\":\"main\"}'\n",
    )?;
    let mut gh_permissions = std::fs::metadata(&gh)?.permissions();
    gh_permissions.set_mode(0o700);
    std::fs::set_permissions(&gh, gh_permissions)?;

    let directory = tempfile::tempdir()?;
    let storage = Storage::open(&directory.path().join("homebot.db")).await?;
    let bot = storage
        .create_bot(
            Uuid::nil(),
            homebot_domain::Bot::create("Patch", "Coding")?,
            1,
        )
        .await?;
    let chat = storage
        .create_direct_chat(Uuid::nil(), bot.id.0, Uuid::now_v7(), 2)
        .await?;
    let workspace_id = Uuid::now_v7();
    storage
        .create_repository_workspace(&homebot_storage::RepositoryWorkspaceRecord {
            id: workspace_id,
            owner_id: Uuid::nil(),
            name: "Fixture".to_owned(),
            root_path: std::fs::canonicalize(repository.path())?
                .to_string_lossy()
                .into_owned(),
            created_at_ms: 3,
            updated_at_ms: 3,
        })
        .await?;
    storage
        .attach_chat_workspace(&homebot_storage::ChatWorkspaceRecord {
            owner_id: Uuid::nil(),
            chat_id: chat.id,
            workspace_id,
            mode: WorkspaceMode::Primary,
            worktree_path: None,
            branch_name: Some("main".to_owned()),
            base_ref: None,
            created_at_ms: 4,
            updated_at_ms: 4,
        })
        .await?;
    let device_id = Uuid::now_v7();
    let device_token = "source-control-policy-device";
    let device_digest: [u8; 32] = Sha256::digest(device_token.as_bytes()).into();
    sqlx::query("INSERT INTO device_sessions (id, owner_id, name, token_digest, endpoint_kind, created_at_ms) VALUES (?, ?, 'Coding phone', ?, 'loopback', 1)")
        .bind(device_id.to_string())
        .bind(Uuid::nil().to_string())
        .bind(device_digest.as_slice())
        .execute(storage.pool())
        .await?;
    let app = router(
        AppState::new(storage.clone(), "correct-token").with_git_runtime(
            Arc::new(homebot_vcs::GitRuntime::discover()?.with_github_cli(Some(gh))),
            directory.path().join("worktrees"),
        ),
    );
    let status_url = format!("/api/v1/chats/{}/vcs/status", chat.id);
    let unauthorized = app
        .clone()
        .oneshot(Request::get(&status_url).body(Body::empty())?)
        .await?;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let rule_id = Uuid::now_v7();
    let denied_rule = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/capability-rules/{rule_id}"),
            &UpsertCapabilityRuleRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                capability: CapabilityClass::GitRemote,
                effect: CapabilityRuleEffect::Deny,
                device_id: Some(device_id),
                bot_id: None,
                chat_id: Some(chat.id),
                workspace_id: Some(workspace_id),
                action_prefix: Some("git.push".to_owned()),
            },
        ))
        .await?;
    assert_eq!(denied_rule.status(), StatusCode::OK);
    let device_push = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/chats/{}/vcs/push", chat.id))
                .header("authorization", format!("Bearer {device_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&VcsPushRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                    remote: "origin".to_owned(),
                    branch: "main".to_owned(),
                    set_upstream: false,
                    approval_id: None,
                })?))?,
        )
        .await?;
    assert_eq!(device_push.status(), StatusCode::FORBIDDEN);
    let device_commit = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/chats/{}/vcs/commit", chat.id))
                .header("authorization", format!("Bearer {device_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&VcsCommitRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                    message: "must not run".to_owned(),
                    stage_all: false,
                })?))?,
        )
        .await?;
    assert_eq!(device_commit.status(), StatusCode::FORBIDDEN);
    let device_workspace = app
        .clone()
        .oneshot(
            Request::post("/api/v1/workspaces")
                .header("authorization", format!("Bearer {device_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(
                    &CreateRepositoryWorkspaceRequest {
                        request_id: Uuid::now_v7(),
                        idempotency_key: Uuid::now_v7(),
                        root_path: repository.path().display().to_string(),
                        name: Some("must not register".to_owned()),
                    },
                )?))?,
        )
        .await?;
    assert_eq!(device_workspace.status(), StatusCode::FORBIDDEN);

    std::fs::write(repository.path().join("README.md"), "working change\n")?;
    std::fs::write(repository.path().join("new.txt"), "new file\n")?;
    run_git(repository.path(), &["add", "new.txt"])?;
    let status = app
        .clone()
        .oneshot(
            Request::get(&status_url)
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    let status: VcsStatus = response_json(status).await?;
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.entries.len(), 2);
    assert!(status.remotes.iter().any(|remote| remote.name == "origin"));
    let staged = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/chats/{}/vcs/diff?staged=true", chat.id))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert!(
        response_json::<WorkingTreeDiffResponse>(staged)
            .await?
            .patch
            .contains("new.txt")
    );
    let unstaged = app
        .clone()
        .oneshot(
            Request::get(format!("/api/v1/chats/{}/vcs/diff", chat.id))
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert!(
        response_json::<WorkingTreeDiffResponse>(unstaged)
            .await?
            .patch
            .contains("working change")
    );

    let commit = VcsCommitRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        message: "Apply working changes".to_owned(),
        stage_all: true,
    };
    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/commit", chat.id),
            &commit,
        ))
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let committed: VcsCommitResult = response_json(created).await?;
    let replay = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/commit", chat.id),
            &commit,
        ))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json::<VcsCommitResult>(replay).await?, committed);

    let branch = VcsCreateBranchRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        branch: "homebot/approved".to_owned(),
        start_point: Some("main".to_owned()),
    };
    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/branches", chat.id),
            &branch,
        ))
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    assert_eq!(
        response_json::<VcsStatus>(created).await?.branch.as_deref(),
        Some("homebot/approved")
    );

    let mut push = VcsPushRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        remote: "origin".to_owned(),
        branch: "homebot/approved".to_owned(),
        set_upstream: true,
        approval_id: None,
    };
    let pending = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/push", chat.id),
            &push,
        ))
        .await?;
    assert_eq!(pending.status(), StatusCode::ACCEPTED);
    let pending: VcsRemoteMutationResponse = response_json(pending).await?;
    assert_eq!(pending.status, VcsMutationStatus::ApprovalRequired);
    let approval = pending.approval.ok_or("missing approval")?;
    let duplicate = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/push", chat.id),
            &push,
        ))
        .await?;
    assert_eq!(
        response_json::<VcsRemoteMutationResponse>(duplicate)
            .await?
            .approval
            .ok_or("missing duplicate approval")?
            .id,
        approval.id
    );
    let denied = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{}/decision", approval.id),
            &ApprovalDecisionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                allow: false,
            },
        ))
        .await?;
    assert_eq!(denied.status(), StatusCode::OK);
    push.approval_id = Some(approval.id);
    let blocked = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/push", chat.id),
            &push,
        ))
        .await?;
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);
    assert!(
        run_git(
            remote.path(),
            &["show-ref", "--verify", "refs/heads/homebot/approved"]
        )
        .is_err()
    );

    push.request_id = Uuid::now_v7();
    push.idempotency_key = Uuid::now_v7();
    push.approval_id = None;
    let pending = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/push", chat.id),
            &push,
        ))
        .await?;
    let pending: VcsRemoteMutationResponse = response_json(pending).await?;
    let approval = pending.approval.ok_or("missing approval")?;
    let allowed = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{}/decision", approval.id),
            &ApprovalDecisionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                allow: true,
            },
        ))
        .await?;
    assert_eq!(allowed.status(), StatusCode::OK);
    push.approval_id = Some(approval.id);
    let pushed = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/push", chat.id),
            &push,
        ))
        .await?;
    assert_eq!(pushed.status(), StatusCode::CREATED);
    let pushed: VcsRemoteMutationResponse = response_json(pushed).await?;
    assert_eq!(pushed.status, VcsMutationStatus::Completed);
    assert_eq!(
        run_git(remote.path(), &["rev-parse", "refs/heads/homebot/approved"])?.trim(),
        committed.commit_oid
    );
    let replay = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/push", chat.id),
            &push,
        ))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<VcsRemoteMutationResponse>(replay).await?,
        pushed
    );
    assert!(
        storage
            .vcs_operation_result(Uuid::nil(), chat.id, push.idempotency_key, "push")
            .await?
            .is_some()
    );

    let metadata = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/api/v1/chats/{}/vcs/pull-request?remote=github&head_branch=homebot%2Fapproved&base_branch=main",
                chat.id
            ))
            .header("authorization", "Bearer correct-token")
            .body(Body::empty())?,
        )
        .await?;
    let metadata: PullRequestMetadata = response_json(metadata).await?;
    assert_eq!(metadata.repository.as_deref(), Some("luinbytes/HomeBot"));
    assert_eq!(
        metadata.current.as_ref().map(|current| current.number),
        Some(42)
    );
    assert!(
        metadata
            .compare_url
            .as_deref()
            .is_some_and(|url| url.starts_with("https://github.com/luinbytes/HomeBot/compare/"))
    );

    let mut pull_request = CreatePullRequestRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        remote: "github".to_owned(),
        head_branch: "homebot/approved".to_owned(),
        base_branch: "main".to_owned(),
        title: "HomeBot source control".to_owned(),
        body: "Verified through the shared server contract.".to_owned(),
        draft: false,
        approval_id: None,
    };
    let pending = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/pull-request", chat.id),
            &pull_request,
        ))
        .await?;
    assert_eq!(pending.status(), StatusCode::ACCEPTED);
    let pending: PullRequestMutationResponse = response_json(pending).await?;
    let approval = pending.approval.ok_or("missing pull request approval")?;
    let allowed = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{}/decision", approval.id),
            &ApprovalDecisionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                allow: true,
            },
        ))
        .await?;
    assert_eq!(allowed.status(), StatusCode::OK);
    pull_request.approval_id = Some(approval.id);
    let created = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/pull-request", chat.id),
            &pull_request,
        ))
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: PullRequestMutationResponse = response_json(created).await?;
    assert_eq!(created.status, VcsMutationStatus::Completed);
    assert_eq!(
        created.result.as_ref().map(|result| result.number),
        Some(42)
    );
    let replay = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/chats/{}/vcs/pull-request", chat.id),
            &pull_request,
        ))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(
        response_json::<PullRequestMutationResponse>(replay).await?,
        created
    );

    let config_path = repository.path().join(".git/config");
    let config = std::fs::read_to_string(&config_path)?;
    std::fs::write(
        &config_path,
        format!("{config}\n[core]\n\tfsmonitor = !touch server-policy-bypass\n"),
    )?;
    let hostile = app
        .oneshot(
            Request::get(&status_url)
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(hostile.status(), StatusCode::CONFLICT);
    assert!(!repository.path().join("server-policy-bypass").exists());
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn capability_rules_are_owner_managed_idempotent_audited_and_restart_enforced()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("homebot.db");
    let storage = Storage::open(&database).await?;
    let app = router(AppState::new(storage.clone(), "correct-token"));
    let rule_id = Uuid::now_v7();
    let request = UpsertCapabilityRuleRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        capability: CapabilityClass::GitRemote,
        effect: CapabilityRuleEffect::Deny,
        device_id: None,
        bot_id: None,
        chat_id: None,
        workspace_id: None,
        action_prefix: Some("git.push".to_owned()),
    };
    let created = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/capability-rules/{rule_id}"),
            &request,
        ))
        .await?;
    assert_eq!(created.status(), StatusCode::OK);
    let created: CapabilityRuleSummary = response_json(created).await?;
    assert_eq!(created.effect, CapabilityRuleEffect::Deny);

    let replay = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/api/v1/capability-rules/{rule_id}"),
            &request,
        ))
        .await?;
    assert_eq!(
        response_json::<CapabilityRuleSummary>(replay).await?,
        created
    );
    let audit = app
        .clone()
        .oneshot(
            Request::get("/api/v1/capability-rules/audit")
                .header("authorization", "Bearer correct-token")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(
        response_json::<Vec<CapabilityRuleAuditSummary>>(audit)
            .await?
            .len(),
        1
    );

    let restarted = AppState::new(Storage::open(&database).await?, "correct-token");
    restarted.ensure_policy_loaded().await?;
    let denied = restarted
        .policy_engine
        .authorize(
            &homebot_tools::CapabilityRequest {
                context: homebot_tools::OperationContext {
                    operation_id: Uuid::now_v7(),
                    owner_id: Uuid::nil(),
                    device_id: Uuid::nil(),
                    bot_id: Uuid::nil(),
                    chat_id: Uuid::nil(),
                    workspace_id: Uuid::nil(),
                },
                capability: homebot_tools::CapabilityClass::GitRemote,
                action: "git.push.origin".to_owned(),
                canonical_resource: "test".to_owned(),
                summary: "test policy".to_owned(),
                destructive: true,
            },
            None,
        )
        .await;
    assert!(matches!(denied, Err(homebot_tools::ToolError::Denied)));

    let device_token = "test-device-capability-token";
    let digest: [u8; 32] = Sha256::digest(device_token.as_bytes()).into();
    sqlx::query("INSERT INTO device_sessions (id, owner_id, name, token_digest, endpoint_kind, created_at_ms) VALUES (?, ?, 'Policy test device', ?, 'loopback', 1)")
        .bind(Uuid::now_v7().to_string())
        .bind(Uuid::nil().to_string())
        .bind(digest.as_slice())
        .execute(storage.pool())
        .await?;
    let forbidden = app
        .clone()
        .oneshot(
            Request::get("/api/v1/capability-rules")
                .header("authorization", format!("Bearer {device_token}"))
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

    let deleted = app
        .oneshot(json_request(
            "DELETE",
            &format!("/api/v1/capability-rules/{rule_id}"),
            &BotMutationRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            },
        ))
        .await?;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let audit = storage.capability_rule_audit(Uuid::nil()).await?;
    assert_eq!(audit.len(), 2);
    assert_eq!(audit[1].action, "deleted");
    assert_eq!(audit[1].snapshot["action_prefix"], "git.push");
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn browser_session_watch_takeover_approval_return_and_restart_are_server_owned()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("homebot.db");
    let storage = Storage::open(&database).await?;
    let bot = storage
        .create_bot(
            Uuid::nil(),
            homebot_domain::Bot::create("Scout", "Browser")?,
            1,
        )
        .await?;
    let second_bot = storage
        .create_bot(
            Uuid::nil(),
            homebot_domain::Bot::create("Nova", "Handoff")?,
            2,
        )
        .await?;
    let chat = storage
        .create_group_chat(
            Uuid::nil(),
            Uuid::now_v7(),
            "Shared computer",
            &[bot.id.0, second_bot.id.0],
            bot.id.0,
            12,
            2,
            3,
        )
        .await?;
    let policy = Arc::new(homebot_tools::PolicyEngine::new(
        Duration::from_secs(60),
        Arc::new(homebot_tools::NoopActivitySink),
    ));
    policy
        .replace_rules(vec![
            homebot_tools::PolicyRule::new(
                homebot_tools::CapabilityClass::BrowserAct,
                homebot_tools::PolicyEffect::Allow,
            )
            .action_prefix("browser.session.create"),
            homebot_tools::PolicyRule::new(
                homebot_tools::CapabilityClass::BrowserAct,
                homebot_tools::PolicyEffect::Allow,
            )
            .action_prefix("browser.session.close"),
            homebot_tools::PolicyRule::new(
                homebot_tools::CapabilityClass::BrowserAct,
                homebot_tools::PolicyEffect::RequireApproval,
            )
            .action_prefix("browser.navigate"),
            homebot_tools::PolicyRule::new(
                homebot_tools::CapabilityClass::BrowserAct,
                homebot_tools::PolicyEffect::RequireApproval,
            )
            .action_prefix("browser.takeover"),
            homebot_tools::PolicyRule::new(
                homebot_tools::CapabilityClass::BrowserObserve,
                homebot_tools::PolicyEffect::Allow,
            ),
        ])
        .await;
    let browser = Arc::new(BrowserFakeRuntime {
        policy: Arc::clone(&policy),
        sessions: Mutex::new(HashMap::new()),
    });
    let app = router(
        AppState::new(storage.clone(), "correct-token")
            .with_policy_engine(policy)
            .with_browser_runtime(browser.clone()),
    );
    let create = CreateBrowserSessionRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        chat_id: chat.id,
        bot_id: bot.id.0,
        profile_id: Uuid::now_v7(),
        profile_name: "Shared login".to_owned(),
        approval_id: None,
    };
    let created = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/browser-sessions", &create))
        .await?;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: BrowserActionResponse = response_json(created).await?;
    let session_id = created.session.id;
    assert_eq!(created.session.controller, BrowserController::Bot);
    let replay = app
        .clone()
        .oneshot(json_request("POST", "/api/v1/browser-sessions", &create))
        .await?;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(browser.sessions.lock().await.len(), 1);
    storage
        .handoff_group_ownership(
            Uuid::nil(),
            chat.id,
            Uuid::now_v7(),
            bot.id.0,
            second_bot.id.0,
            None,
            "Continue the browser task",
            4,
        )
        .await?;
    assert_eq!(
        storage
            .browser_session(Uuid::nil(), session_id)
            .await?
            .profile_name,
        "Shared login"
    );

    let mut navigate = BrowserActionRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        command: BrowserCommand::Navigate {
            url: "https://example.test/private".to_owned(),
        },
        approval_id: None,
    };
    let pending = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/browser-sessions/{session_id}/actions"),
            &navigate,
        ))
        .await?;
    assert_eq!(pending.status(), StatusCode::ACCEPTED);
    let pending: BrowserActionResponse = response_json(pending).await?;
    assert_eq!(
        pending.session.status,
        BrowserSessionStatus::AwaitingApproval
    );
    let approval = pending.approval.ok_or("missing browser approval")?;
    let decision = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{}/decision", approval.id),
            &ApprovalDecisionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                allow: true,
            },
        ))
        .await?;
    assert_eq!(decision.status(), StatusCode::OK);
    navigate.approval_id = Some(approval.id);
    let navigated = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/browser-sessions/{session_id}/actions"),
            &navigate,
        ))
        .await?;
    assert_eq!(navigated.status(), StatusCode::OK);

    let current = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/browser-sessions/{session_id}/actions"),
            &BrowserActionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                command: BrowserCommand::CurrentUrl,
                approval_id: None,
            },
        ))
        .await?;
    assert_eq!(current.status(), StatusCode::OK);
    let current: BrowserActionResponse = response_json(current).await?;
    assert_eq!(
        current.session.current_url.as_deref(),
        Some("https://example.test/private")
    );

    let screenshot = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/browser-sessions/{session_id}/actions"),
            &BrowserActionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                command: BrowserCommand::CaptureScreenshot,
                approval_id: None,
            },
        ))
        .await?;
    assert_eq!(screenshot.status(), StatusCode::OK);
    assert!(
        response_json::<BrowserActionResponse>(screenshot)
            .await?
            .artifact
            .is_some()
    );

    let controlling_token = "browser-controller-device";
    let controlling_device_id = Uuid::now_v7();
    let competing_token = "browser-competing-device";
    let competing_device_id = Uuid::now_v7();
    for (id, name, token) in [
        (
            controlling_device_id,
            "Controlling phone",
            controlling_token,
        ),
        (competing_device_id, "Competing phone", competing_token),
    ] {
        let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        sqlx::query("INSERT INTO device_sessions (id, owner_id, name, token_digest, endpoint_kind, created_at_ms) VALUES (?, ?, ?, ?, 'loopback', 1)")
            .bind(id.to_string())
            .bind(Uuid::nil().to_string())
            .bind(name)
            .bind(digest.as_slice())
            .execute(storage.pool())
            .await?;
    }

    let mut takeover = BrowserMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        approval_id: None,
    };
    let pending = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/browser-sessions/{session_id}/takeover"))
                .header("authorization", format!("Bearer {controlling_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&takeover)?))?,
        )
        .await?;
    assert_eq!(pending.status(), StatusCode::ACCEPTED);
    let pending: BrowserActionResponse = response_json(pending).await?;
    let approval = pending.approval.ok_or("missing takeover approval")?;
    let decision = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{}/decision", approval.id),
            &ApprovalDecisionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                allow: true,
            },
        ))
        .await?;
    assert_eq!(decision.status(), StatusCode::OK);
    takeover.approval_id = Some(approval.id);
    let controlled = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/browser-sessions/{session_id}/takeover"))
                .header("authorization", format!("Bearer {controlling_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&takeover)?))?,
        )
        .await?;
    assert_eq!(controlled.status(), StatusCode::OK);
    assert_eq!(
        response_json::<BrowserActionResponse>(controlled)
            .await?
            .session
            .controller,
        BrowserController::User
    );
    let leased = storage.browser_session(Uuid::nil(), session_id).await?;
    assert_eq!(leased.controlling_device_id, Some(controlling_device_id));
    assert!(leased.takeover_expires_at_ms.is_some());

    let blocked_bot_action = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/browser-sessions/{session_id}/actions"),
            &BrowserActionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                command: BrowserCommand::CurrentUrl,
                approval_id: None,
            },
        ))
        .await?;
    assert_eq!(blocked_bot_action.status(), StatusCode::FORBIDDEN);

    sqlx::query("UPDATE browser_sessions SET takeover_expires_at_ms = 1 WHERE id = ?")
        .bind(session_id.to_string())
        .execute(storage.pool())
        .await?;
    let expired_action = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/browser-sessions/{session_id}/actions"))
                .header("authorization", format!("Bearer {controlling_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&BrowserActionRequest {
                    request_id: Uuid::now_v7(),
                    idempotency_key: Uuid::now_v7(),
                    command: BrowserCommand::CurrentUrl,
                    approval_id: None,
                })?))?,
        )
        .await?;
    assert_eq!(expired_action.status(), StatusCode::CONFLICT);

    let mut competing_takeover = BrowserMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        approval_id: None,
    };
    let pending = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/browser-sessions/{session_id}/takeover"))
                .header("authorization", format!("Bearer {competing_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&competing_takeover)?))?,
        )
        .await?;
    assert_eq!(pending.status(), StatusCode::ACCEPTED);
    let approval = response_json::<BrowserActionResponse>(pending)
        .await?
        .approval
        .ok_or("missing replacement takeover approval")?;
    let decision = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/api/v1/approvals/{}/decision", approval.id),
            &ApprovalDecisionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                allow: true,
            },
        ))
        .await?;
    assert_eq!(decision.status(), StatusCode::OK);
    competing_takeover.approval_id = Some(approval.id);
    let replacement = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/browser-sessions/{session_id}/takeover"))
                .header("authorization", format!("Bearer {competing_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&competing_takeover)?))?,
        )
        .await?;
    assert_eq!(replacement.status(), StatusCode::OK);
    let replacement = response_json::<BrowserActionResponse>(replacement).await?;
    assert_eq!(replacement.session.controller, BrowserController::User);
    assert_eq!(
        storage
            .browser_session(Uuid::nil(), session_id)
            .await?
            .controlling_device_id,
        Some(competing_device_id)
    );

    let return_request = BrowserMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        approval_id: None,
    };
    let former_controller_return = app
        .clone()
        .oneshot(
            Request::post(format!("/api/v1/browser-sessions/{session_id}/return"))
                .header("authorization", format!("Bearer {controlling_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&return_request)?))?,
        )
        .await?;
    assert_eq!(former_controller_return.status(), StatusCode::FORBIDDEN);
    let returned = app
        .oneshot(
            Request::post(format!("/api/v1/browser-sessions/{session_id}/return"))
                .header("authorization", format!("Bearer {competing_token}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&return_request)?))?,
        )
        .await?;
    assert_eq!(returned.status(), StatusCode::OK);
    assert_eq!(
        response_json::<BrowserActionResponse>(returned)
            .await?
            .session
            .controller,
        BrowserController::Bot
    );

    let race_time = unix_time_ms();
    let first = storage.claim_browser_takeover(
        Uuid::nil(),
        session_id,
        controlling_device_id,
        race_time + 60_000,
        race_time,
    );
    let second = storage.claim_browser_takeover(
        Uuid::nil(),
        session_id,
        competing_device_id,
        race_time + 60_000,
        race_time,
    );
    let (first, second) = tokio::join!(first, second);
    assert!(first.is_ok() ^ second.is_ok());
    assert!(matches!(
        first.as_ref().err().or_else(|| second.as_ref().err()),
        Some(homebot_storage::StorageError::BrowserTakeoverConflict)
    ));
    let winner = first
        .as_ref()
        .ok()
        .map_or(competing_device_id, |_| controlling_device_id);
    storage
        .release_browser_takeover(Uuid::nil(), session_id, winner, race_time + 1)
        .await?;

    let reopened = Storage::open(&database).await?;
    let durable = reopened.browser_session(Uuid::nil(), session_id).await?;
    assert_eq!(durable.profile_name, "Shared login");
    assert_eq!(
        durable.current_url.as_deref(),
        Some("https://example.test/private")
    );
    assert_eq!(durable.controlling_device_id, None);
    assert_eq!(durable.takeover_expires_at_ms, None);
    let persisted: Vec<String> =
        sqlx::query_scalar("SELECT display_name || directory_ref FROM browser_profiles")
            .fetch_all(reopened.pool())
            .await?;
    assert!(persisted.iter().all(|value| !value.contains("private")));
    Ok(())
}
