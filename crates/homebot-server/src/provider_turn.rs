//! Provider-neutral execution of one direct-chat Bot turn.

use homebot_domain::{
    Bot,
    chat::{
        ActivityStatus, ApprovalStatus, ChatApproval, DirectChat, ExecutionActivity,
        GroupBotStatus, MessageStatus,
    },
};
use homebot_protocol::{
    CheckpointPhase, ErrorCode, ErrorEnvelope, InteractionMode, ServerEventBody,
};
use homebot_providers::{
    ActivityStatus as ProviderActivityStatus, ApprovalDecision, ProviderAdapterId,
    ProviderAttachment, ProviderError, ProviderErrorCode, ProviderEvent, ProviderRun,
    ResumeRequest, StartRequest,
};
use uuid::Uuid;

use crate::{
    AppState, ChatOperation,
    bots::ApiError,
    chats::{activity_summary, approval_summary, message_summary, publish},
    unix_time_ms,
};

#[allow(clippy::too_many_lines)]
pub(super) fn start_if_configured<'a>(
    state: &'a AppState,
    chat: &'a DirectChat,
    prompt: &'a str,
    attachment_ids: &'a [Uuid],
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, ApiError>> + Send + 'a>> {
    start(state, chat.id, chat.bot_id, prompt, attachment_ids, false)
}

pub(super) fn start_group_if_configured<'a>(
    state: &'a AppState,
    chat_id: Uuid,
    bot_id: Uuid,
    prompt: &'a str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, ApiError>> + Send + 'a>> {
    start(state, chat_id, bot_id, prompt, &[], true)
}

#[allow(clippy::too_many_lines)]
fn start<'a>(
    state: &'a AppState,
    chat_id: Uuid,
    bot_id: Uuid,
    prompt: &'a str,
    attachment_ids: &'a [Uuid],
    group: bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, ApiError>> + Send + 'a>> {
    Box::pin(async move {
        let Some(route) = state
            .storage
            .provider_route_for_bot(state.owner_id, bot_id)
            .await?
        else {
            return Ok(false);
        };
        let bot = state.storage.get_bot(state.owner_id, bot_id).await?;
        let prompt = prompt_with_bot(&bot, prompt);
        let adapter_id =
            ProviderAdapterId::new(route.adapter_kind).map_err(|_| ApiError::internal())?;
        let operation_id = Uuid::now_v7();
        let message_id = Uuid::now_v7();
        let attachments = provider_attachments(state, attachment_ids).await?;
        let tools = if group {
            crate::groups::provider_tools(state, chat_id, bot_id).await?
        } else {
            Vec::new()
        };
        let conversation = state
            .storage
            .provider_conversation(bot_id, chat_id, route.profile_id)
            .await?;
        let operation = ChatOperation {
            operation: operation_id,
            chat: chat_id,
            adapter: adapter_id.clone(),
            profile: route.profile_id,
            bot: bot_id,
            message: message_id,
            group,
        };
        if group {
            prepare_group_turn(state, &operation).await?;
        }
        let assistant = state
            .storage
            .create_bot_message(state.owner_id, chat_id, bot_id, message_id, unix_time_ms())
            .await?;
        publish(
            state,
            "message_changed",
            ServerEventBody::MessageChanged {
                message: message_summary(state, assistant).await?,
            },
        )
        .await?;
        if crate::checkpoints::capture_for_turn(
            state,
            chat_id,
            message_id,
            route.profile_id,
            conversation.clone(),
            CheckpointPhase::BeforeTurn,
        )
        .await
        .is_err()
        {
            finish_failed_start(
                state,
                chat_id,
                operation,
                checkpoint_error("The coding workspace could not be checkpointed before this turn"),
            )
            .await?;
            return Ok(true);
        }
        let mode = if group {
            InteractionMode::Default
        } else {
            crate::working_context::summary(state, chat_id)
                .await?
                .map_or(InteractionMode::Default, |context| context.interaction_mode)
        };
        let working_directory = provider_working_directory(state, chat_id, bot_id).await?;
        let result = if let Some(conversation_id) = conversation {
            state
                .provider_runtime
                .resume(
                    &adapter_id,
                    ResumeRequest {
                        operation_id,
                        conversation_id,
                        prompt: prompt.clone(),
                        model: route.model.clone(),
                        working_directory: working_directory.clone(),
                        mode: crate::working_context::execution_mode(mode),
                        attachments,
                        tools,
                    },
                )
                .await
        } else {
            state
                .provider_runtime
                .start(
                    &adapter_id,
                    StartRequest {
                        operation_id,
                        bot_id,
                        chat_id,
                        prompt,
                        model: route.model.clone(),
                        working_directory,
                        mode: crate::working_context::execution_mode(mode),
                        attachments,
                        tools,
                    },
                )
                .await
        };
        let run = match result {
            Ok(run) => run,
            Err(error) => {
                finish_failed_start(state, chat_id, operation, provider_error(&error)).await?;
                return Ok(true);
            }
        };
        state
            .chat_operations
            .lock()
            .await
            .insert(operation_id, operation.clone());
        let state = state.clone();
        tokio::spawn(async move {
            if consume(state.clone(), operation.clone(), run)
                .await
                .is_err()
                && state
                    .chat_operations
                    .lock()
                    .await
                    .contains_key(&operation_id)
            {
                let error = ErrorEnvelope {
                    code: ErrorCode::Internal,
                    message: "The Bot turn ended unexpectedly".to_owned(),
                    retryable: true,
                    request_id: None,
                    retry_after_ms: None,
                    details: None,
                };
                let _ = finish(
                    &state,
                    chat_id,
                    operation,
                    MessageStatus::Failed,
                    Some(error),
                )
                .await;
            }
        });
        Ok(true)
    })
}

async fn prepare_group_turn(state: &AppState, operation: &ChatOperation) -> Result<(), ApiError> {
    let now = unix_time_ms();
    let participant = state
        .storage
        .set_group_bot_status(
            state.owner_id,
            operation.chat,
            operation.bot,
            GroupBotStatus::Running,
            Some(operation.operation),
            now,
        )
        .await?;
    let group = match state
        .storage
        .record_group_coordination_turn(state.owner_id, operation.chat, now)
        .await
    {
        Ok(group) => group,
        Err(error) => {
            let _ = state
                .storage
                .set_group_bot_status(
                    state.owner_id,
                    operation.chat,
                    operation.bot,
                    GroupBotStatus::Idle,
                    None,
                    now,
                )
                .await;
            return Err(error.into());
        }
    };
    publish(
        state,
        "group_participant_changed",
        ServerEventBody::GroupParticipantChanged {
            participant: crate::groups::participant_summary(&participant),
        },
    )
    .await?;
    publish(
        state,
        "group_chat_changed",
        ServerEventBody::GroupChatChanged {
            group: crate::groups::group_summary(group),
        },
    )
    .await?;
    Ok(())
}

async fn provider_working_directory(
    state: &AppState,
    chat_id: Uuid,
    bot_id: Uuid,
) -> Result<Option<std::path::PathBuf>, ApiError> {
    let Some(workspace) = state
        .storage
        .chat_workspace(state.owner_id, chat_id)
        .await?
    else {
        let directory = state
            .artifact_root
            .join("bot-workspaces")
            .join(bot_id.to_string());
        tokio::fs::create_dir_all(&directory)
            .await
            .map_err(|_| ApiError::internal())?;
        return Ok(Some(directory));
    };
    let repository = state
        .storage
        .repository_workspace(state.owner_id, workspace.workspace_id)
        .await?;
    Ok(Some(
        workspace
            .worktree_path
            .unwrap_or(repository.root_path)
            .into(),
    ))
}

fn prompt_with_bot(bot: &Bot, prompt: &str) -> String {
    let responsibility = if bot.description.is_empty() {
        String::new()
    } else {
        format!("\nResponsibility: {}", bot.description)
    };
    format!(
        "<homebot_bot>\nName: {}\nRole: {}{responsibility}\nUse this identity and responsibility for this turn.\n</homebot_bot>\n\n{prompt}",
        bot.name, bot.title
    )
}

pub(super) async fn cancel(state: &AppState, chat_id: Uuid) -> Result<(), ApiError> {
    let operation = state
        .chat_operations
        .lock()
        .await
        .values()
        .find(|operation| operation.chat == chat_id)
        .cloned();
    if let Some(operation) = operation {
        state
            .provider_runtime
            .cancel(operation.operation)
            .await
            .map_err(|_| ApiError::internal())?;
    }
    Ok(())
}

pub(super) async fn cancel_group(state: &AppState, chat_id: Uuid) -> Result<(), ApiError> {
    let operations = state
        .chat_operations
        .lock()
        .await
        .values()
        .filter(|operation| operation.chat == chat_id)
        .cloned()
        .collect::<Vec<_>>();
    for operation in operations {
        state
            .provider_runtime
            .cancel(operation.operation)
            .await
            .map_err(|_| ApiError::internal())?;
    }
    Ok(())
}

pub(super) async fn resolve_approval(
    state: &AppState,
    approval_id: Uuid,
    allow: bool,
) -> Result<(), ApiError> {
    let approval = state
        .storage
        .chat_approval(state.owner_id, approval_id)
        .await?;
    if approval.capability.starts_with("homebot.git.")
        || approval.capability.starts_with("homebot.browser.")
    {
        state.ensure_policy_loaded().await?;
        state
            .policy_engine
            .decide(
                approval_id,
                if allow {
                    homebot_tools::ApprovalDecision::AllowOnce
                } else {
                    homebot_tools::ApprovalDecision::Deny
                },
            )
            .await
            .map_err(|_| ApiError::conflict("The capability approval is no longer active"))?;
        return Ok(());
    }
    let operation = state
        .chat_operations
        .lock()
        .await
        .values()
        .find(|operation| operation.operation == approval.operation_id)
        .cloned();
    let Some(operation) = operation else {
        return if allow {
            Err(ApiError::conflict(
                "The provider operation is no longer active",
            ))
        } else {
            Ok(())
        };
    };
    state
        .provider_runtime
        .resolve_approval(
            &operation.adapter,
            approval_id,
            if allow {
                ApprovalDecision::AllowOnce
            } else {
                ApprovalDecision::Deny
            },
        )
        .await
        .map_err(|_| ApiError::internal())
}

#[allow(clippy::too_many_lines)]
async fn consume(
    state: AppState,
    operation: ChatOperation,
    mut run: ProviderRun,
) -> Result<(), ApiError> {
    let chat_id = operation.chat;
    while let Some(event) = run.events.recv().await {
        match event {
            ProviderEvent::ConversationStarted { conversation_id }
            | ProviderEvent::Compacted { conversation_id } => {
                state
                    .storage
                    .set_provider_conversation(
                        operation.bot,
                        chat_id,
                        operation.profile,
                        &conversation_id,
                    )
                    .await?;
            }
            ProviderEvent::ContentDelta { text } => {
                state
                    .storage
                    .append_bot_message_delta(state.owner_id, operation.message, &text)
                    .await?;
                publish(
                    &state,
                    "message_delta",
                    ServerEventBody::MessageDelta {
                        chat_id,
                        message_id: operation.message,
                        delta: text,
                    },
                )
                .await?;
            }
            ProviderEvent::Activity { activity } => {
                let now = unix_time_ms();
                let status = map_activity_status(activity.status);
                let activity = ExecutionActivity {
                    id: activity.activity_id,
                    chat_id,
                    message_id: Some(operation.message),
                    kind: format!("{:?}", activity.kind).to_ascii_lowercase(),
                    title: activity.title.clone(),
                    detail: activity.title,
                    presentation_json: serde_json::json!({
                        "risk": "low",
                        "detail": {
                            "kind": "generic",
                            "summary": "Provider activity"
                        },
                        "copy_text": null,
                        "open_artifact_id": null
                    }),
                    status,
                    requires_attention: matches!(status, ActivityStatus::Failed),
                    started_at_ms: now,
                    finished_at_ms: (!matches!(status, ActivityStatus::Running)).then_some(now),
                };
                state
                    .storage
                    .upsert_activity(state.owner_id, &activity)
                    .await?;
                publish(
                    &state,
                    "activity_changed",
                    ServerEventBody::ActivityChanged {
                        activity: activity_summary(activity),
                    },
                )
                .await?;
            }
            ProviderEvent::ApprovalRequired { approval } => {
                let approval = ChatApproval {
                    id: approval.approval_id,
                    owner_id: state.owner_id,
                    chat_id,
                    message_id: Some(operation.message),
                    operation_id: operation.operation,
                    capability: approval.capability,
                    title: approval.action,
                    detail: format!("{}: {}", approval.resource, approval.reason),
                    status: ApprovalStatus::Pending,
                    created_at_ms: unix_time_ms(),
                    decided_at_ms: None,
                };
                state.storage.create_chat_approval(&approval).await?;
                publish(
                    &state,
                    "approval_changed",
                    ServerEventBody::ApprovalChanged {
                        approval: approval_summary(approval),
                    },
                )
                .await?;
            }
            ProviderEvent::ToolCall { call } => {
                let result = if operation.group {
                    crate::groups::handle_provider_tool(
                        &state,
                        chat_id,
                        operation.bot,
                        operation.message,
                        &call,
                    )
                    .await
                } else {
                    homebot_providers::ProviderToolResult {
                        success: false,
                        content: "HomeBot collaboration tools require a group chat".to_owned(),
                    }
                };
                state
                    .provider_runtime
                    .resolve_tool_call(&operation.adapter, call.call_id, result)
                    .await
                    .map_err(|_| ApiError::internal())?;
            }
            ProviderEvent::Usage { usage } => {
                if !operation.group {
                    state
                        .storage
                        .update_working_context_usage(
                            state.owner_id,
                            chat_id,
                            usage.input_tokens.saturating_add(usage.output_tokens),
                            None,
                            unix_time_ms(),
                        )
                        .await?;
                    if let Some(context) = crate::working_context::summary(&state, chat_id).await? {
                        publish(
                            &state,
                            "working_context_changed",
                            ServerEventBody::WorkingContextChanged { context },
                        )
                        .await?;
                    }
                }
            }
            ProviderEvent::Completed => {
                finish(&state, chat_id, operation, MessageStatus::Completed, None).await?;
                return Ok(());
            }
            ProviderEvent::Cancelled => {
                finish(&state, chat_id, operation, MessageStatus::Cancelled, None).await?;
                return Ok(());
            }
            ProviderEvent::Failed { error } => {
                let error = provider_error(&error);
                finish(
                    &state,
                    chat_id,
                    operation,
                    MessageStatus::Failed,
                    Some(error),
                )
                .await?;
                return Ok(());
            }
        }
    }
    Err(ApiError::internal())
}

#[allow(clippy::too_many_lines)]
async fn finish(
    state: &AppState,
    chat_id: Uuid,
    operation: ChatOperation,
    mut status: MessageStatus,
    mut error: Option<ErrorEnvelope>,
) -> Result<(), ApiError> {
    let conversation = state
        .storage
        .provider_conversation(operation.bot, chat_id, operation.profile)
        .await?;
    if crate::checkpoints::capture_for_turn(
        state,
        chat_id,
        operation.message,
        operation.profile,
        conversation,
        CheckpointPhase::AfterTurn,
    )
    .await
    .is_err()
    {
        status = MessageStatus::Failed;
        error = Some(checkpoint_error(
            "The Bot finished, but HomeBot could not checkpoint the coding workspace",
        ));
    }
    let error_json = error
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| ApiError::internal())?;
    let now = unix_time_ms();
    finish_interactions(state, chat_id, &operation, now).await?;
    let message = state
        .storage
        .finish_bot_message(
            state.owner_id,
            operation.message,
            status,
            error_json.as_ref(),
            now,
        )
        .await?;
    if !operation.group && matches!(status, MessageStatus::Completed | MessageStatus::Failed) {
        let _ = state
            .storage
            .increment_chat_unread(state.owner_id, chat_id, now)
            .await?;
    }
    publish(
        state,
        "message_changed",
        ServerEventBody::MessageChanged {
            message: message_summary(state, message).await?,
        },
    )
    .await?;
    if operation.group {
        let group = state
            .storage
            .get_group_chat(state.owner_id, chat_id)
            .await?;
        let participant = state
            .storage
            .set_group_bot_status(
                state.owner_id,
                chat_id,
                operation.bot,
                if group.stop_requested || status == MessageStatus::Cancelled {
                    GroupBotStatus::Stopped
                } else if status == MessageStatus::Completed {
                    GroupBotStatus::Completed
                } else {
                    GroupBotStatus::Failed
                },
                None,
                now,
            )
            .await?;
        publish(
            state,
            "group_participant_changed",
            ServerEventBody::GroupParticipantChanged {
                participant: crate::groups::participant_summary(&participant),
            },
        )
        .await?;
    } else {
        let chat = state
            .storage
            .set_chat_running(state.owner_id, chat_id, false, now)
            .await?;
        publish(
            state,
            "chat_changed",
            ServerEventBody::ChatChanged {
                chat: crate::chats::chat_summary(chat),
            },
        )
        .await?;
    }
    if matches!(status, MessageStatus::Completed | MessageStatus::Failed) {
        let bot = state.storage.get_bot(state.owner_id, operation.bot).await?;
        publish(
            state,
            "bot_changed",
            ServerEventBody::BotChanged {
                bot: crate::bots::summary(state, bot).await,
            },
        )
        .await?;
    }
    state.provider_runtime.finish(operation.operation).await;
    state
        .chat_operations
        .lock()
        .await
        .remove(&operation.operation);
    if !operation.group && status == MessageStatus::Completed {
        start_next_queued(state, chat_id).await?;
    }
    Ok(())
}

async fn finish_interactions(
    state: &AppState,
    chat_id: Uuid,
    operation: &ChatOperation,
    now: i64,
) -> Result<(), ApiError> {
    let (activities, approvals) = state
        .storage
        .finish_provider_interactions(
            state.owner_id,
            chat_id,
            operation.message,
            operation.operation,
            now,
        )
        .await?;
    for activity in activities {
        publish(
            state,
            "activity_changed",
            ServerEventBody::ActivityChanged {
                activity: activity_summary(activity),
            },
        )
        .await?;
    }
    for approval in approvals {
        publish(
            state,
            "approval_changed",
            ServerEventBody::ApprovalChanged {
                approval: approval_summary(approval),
            },
        )
        .await?;
    }
    Ok(())
}

async fn start_next_queued(state: &AppState, chat_id: Uuid) -> Result<(), ApiError> {
    let Some(promoted) = state
        .storage
        .promote_next_queued_prompt(state.owner_id, chat_id, unix_time_ms())
        .await?
    else {
        return Ok(());
    };
    publish(
        state,
        "queued_prompt_removed",
        ServerEventBody::QueuedPromptRemoved {
            chat_id,
            prompt_id: promoted.prompt.id,
        },
    )
    .await?;
    for prompt in state
        .storage
        .queued_prompts(state.owner_id, chat_id)
        .await?
    {
        publish(
            state,
            "queued_prompt_changed",
            ServerEventBody::QueuedPromptChanged {
                prompt: crate::chats::prompt_summary(prompt),
            },
        )
        .await?;
    }
    publish(
        state,
        "message_changed",
        ServerEventBody::MessageChanged {
            message: message_summary(state, promoted.message).await?,
        },
    )
    .await?;
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    publish(
        state,
        "chat_changed",
        ServerEventBody::ChatChanged {
            chat: crate::chats::chat_summary(chat.clone()),
        },
    )
    .await?;
    let prompt =
        crate::chats::prompt_with_skills(&promoted.prompt.content, &promoted.applied_skills)?;
    let started = start_if_configured(state, &chat, &prompt, &promoted.prompt.attachment_ids).await;
    if !matches!(started, Ok(true)) {
        let chat = state
            .storage
            .set_chat_running(state.owner_id, chat_id, false, unix_time_ms())
            .await?;
        publish(
            state,
            "chat_changed",
            ServerEventBody::ChatChanged {
                chat: crate::chats::chat_summary(chat),
            },
        )
        .await?;
    }
    started.map(|_| ())
}

fn checkpoint_error(message: &str) -> ErrorEnvelope {
    ErrorEnvelope {
        code: ErrorCode::Internal,
        message: message.to_owned(),
        retryable: true,
        request_id: None,
        retry_after_ms: None,
        details: None,
    }
}

async fn finish_failed_start(
    state: &AppState,
    chat_id: Uuid,
    operation: ChatOperation,
    error: ErrorEnvelope,
) -> Result<(), ApiError> {
    finish(
        state,
        chat_id,
        operation,
        MessageStatus::Failed,
        Some(error),
    )
    .await
}

async fn provider_attachments(
    state: &AppState,
    ids: &[Uuid],
) -> Result<Vec<ProviderAttachment>, ApiError> {
    let mut attachments = Vec::with_capacity(ids.len());
    for id in ids {
        let attachment = state
            .storage
            .attachment(*id, state.owner_id)
            .await?
            .ok_or(homebot_storage::StorageError::AttachmentUnavailable)?;
        attachments.push(ProviderAttachment {
            attachment_id: attachment.id,
            media_type: attachment.media_type,
            size_bytes: attachment.size_bytes,
        });
    }
    Ok(attachments)
}

fn map_activity_status(status: ProviderActivityStatus) -> ActivityStatus {
    match status {
        ProviderActivityStatus::Started | ProviderActivityStatus::Updated => {
            ActivityStatus::Running
        }
        ProviderActivityStatus::Completed => ActivityStatus::Succeeded,
        ProviderActivityStatus::Failed => ActivityStatus::Failed,
        ProviderActivityStatus::Cancelled => ActivityStatus::Cancelled,
    }
}

fn provider_error(error: &impl ProviderErrorView) -> ErrorEnvelope {
    ErrorEnvelope {
        code: match error.code() {
            ProviderErrorCode::NotInstalled
            | ProviderErrorCode::AuthenticationRequired
            | ProviderErrorCode::Unavailable => ErrorCode::ProviderUnavailable,
            ProviderErrorCode::Cancelled => ErrorCode::OperationCancelled,
            ProviderErrorCode::InvalidRequest | ProviderErrorCode::ConversationUnavailable => {
                ErrorCode::ValidationFailed
            }
            ProviderErrorCode::ProtocolViolation
            | ProviderErrorCode::ProcessCrashed
            | ProviderErrorCode::TimedOut
            | ProviderErrorCode::Internal => ErrorCode::Internal,
        },
        message: error.message().to_owned(),
        retryable: error.retryable(),
        request_id: None,
        retry_after_ms: None,
        details: None,
    }
}

trait ProviderErrorView {
    fn code(&self) -> ProviderErrorCode;
    fn message(&self) -> &str;
    fn retryable(&self) -> bool;
}

impl ProviderErrorView for ProviderError {
    fn code(&self) -> ProviderErrorCode {
        self.code
    }

    fn message(&self) -> &str {
        &self.message
    }

    fn retryable(&self) -> bool {
        self.retryable
    }
}

impl ProviderErrorView for homebot_providers::ProviderRuntimeError {
    fn code(&self) -> ProviderErrorCode {
        match self {
            Self::Provider(error) => error.code,
            Self::AdapterNotFound(_) => ProviderErrorCode::NotInstalled,
            _ => ProviderErrorCode::Internal,
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Provider(error) => &error.message,
            Self::AdapterNotFound(_) => "The configured provider is not available",
            _ => "The provider could not start this turn",
        }
    }

    fn retryable(&self) -> bool {
        matches!(self, Self::Provider(error) if error.retryable)
    }
}
