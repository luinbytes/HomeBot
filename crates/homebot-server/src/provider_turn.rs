//! Provider-neutral execution of one direct-chat Bot turn.

use homebot_domain::chat::{
    ActivityStatus, ApprovalStatus, ChatApproval, DirectChat, ExecutionActivity, MessageStatus,
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
    Box::pin(async move {
        let Some(route) = state
            .storage
            .provider_route_for_bot(state.owner_id, chat.bot_id)
            .await?
        else {
            return Ok(false);
        };
        let adapter_id =
            ProviderAdapterId::new(route.adapter_kind).map_err(|_| ApiError::internal())?;
        let operation_id = Uuid::now_v7();
        let message_id = Uuid::now_v7();
        let attachments = provider_attachments(state, attachment_ids).await?;
        let conversation = state
            .storage
            .provider_conversation(chat.bot_id, chat.id, route.profile_id)
            .await?;
        let assistant = state
            .storage
            .create_bot_message(
                state.owner_id,
                chat.id,
                chat.bot_id,
                message_id,
                unix_time_ms(),
            )
            .await?;
        publish(
            state,
            "message_changed",
            ServerEventBody::MessageChanged {
                message: message_summary(state, assistant).await?,
            },
        )
        .await?;
        let operation = ChatOperation {
            operation: operation_id,
            adapter: adapter_id.clone(),
            profile: route.profile_id,
            bot: chat.bot_id,
            message: message_id,
        };
        if crate::checkpoints::capture_for_turn(
            state,
            chat.id,
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
                chat.id,
                operation,
                checkpoint_error("The coding workspace could not be checkpointed before this turn"),
            )
            .await?;
            return Ok(true);
        }
        let mode = crate::working_context::summary(state, chat.id)
            .await?
            .map_or(InteractionMode::Default, |context| context.interaction_mode);
        let result = if let Some(conversation_id) = conversation {
            state
                .provider_runtime
                .resume(
                    &adapter_id,
                    ResumeRequest {
                        operation_id,
                        conversation_id,
                        prompt: prompt.to_owned(),
                        model: route.model.clone(),
                        mode: crate::working_context::execution_mode(mode),
                        attachments,
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
                        bot_id: chat.bot_id,
                        chat_id: chat.id,
                        prompt: prompt.to_owned(),
                        model: route.model.clone(),
                        mode: crate::working_context::execution_mode(mode),
                        attachments,
                    },
                )
                .await
        };
        let run = match result {
            Ok(run) => run,
            Err(error) => {
                finish_failed_start(state, chat.id, operation, provider_error(&error)).await?;
                return Ok(true);
            }
        };
        state
            .chat_operations
            .lock()
            .await
            .insert(chat.id, operation);
        let state = state.clone();
        let chat_id = chat.id;
        tokio::spawn(async move {
            if consume(state.clone(), chat_id, run).await.is_err() {
                let error = ErrorEnvelope {
                    code: ErrorCode::Internal,
                    message: "The Bot turn ended unexpectedly".to_owned(),
                    retryable: true,
                    request_id: None,
                    retry_after_ms: None,
                    details: None,
                };
                let operation = state.chat_operations.lock().await.get(&chat_id).cloned();
                if let Some(operation) = operation {
                    let _ = finish(
                        &state,
                        chat_id,
                        operation,
                        MessageStatus::Failed,
                        Some(error),
                    )
                    .await;
                }
            }
        });
        Ok(true)
    })
}

pub(super) async fn cancel(state: &AppState, chat_id: Uuid) -> Result<(), ApiError> {
    let operation = state.chat_operations.lock().await.get(&chat_id).cloned();
    if let Some(operation) = operation {
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
    if approval.capability.starts_with("homebot.git.") {
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
            .map_err(|_| ApiError::conflict("The Git approval is no longer active"))?;
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
async fn consume(state: AppState, chat_id: Uuid, mut run: ProviderRun) -> Result<(), ApiError> {
    while let Some(event) = run.events.recv().await {
        let operation = state
            .chat_operations
            .lock()
            .await
            .get(&chat_id)
            .cloned()
            .ok_or_else(ApiError::internal)?;
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
            ProviderEvent::Usage { usage } => {
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
    let message = state
        .storage
        .finish_bot_message(
            state.owner_id,
            operation.message,
            status,
            error_json.as_ref(),
            unix_time_ms(),
        )
        .await?;
    if matches!(status, MessageStatus::Completed | MessageStatus::Failed) {
        let _ = state
            .storage
            .increment_chat_unread(state.owner_id, chat_id, unix_time_ms())
            .await?;
    }
    let chat = state
        .storage
        .set_chat_running(state.owner_id, chat_id, false, unix_time_ms())
        .await?;
    publish(
        state,
        "message_changed",
        ServerEventBody::MessageChanged {
            message: message_summary(state, message).await?,
        },
    )
    .await?;
    publish(
        state,
        "chat_changed",
        ServerEventBody::ChatChanged {
            chat: crate::chats::chat_summary(chat),
        },
    )
    .await?;
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
    state.chat_operations.lock().await.remove(&chat_id);
    if status == MessageStatus::Completed {
        start_next_queued(state, chat_id).await?;
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
