use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_domain::chat::{
    ActivityStatus as DomainActivityStatus, ApprovalStatus as DomainApprovalStatus,
    ChatApproval as DomainApproval, ChatMessage as DomainMessage, DirectChat as DomainChat,
    ExecutionActivity as DomainActivity, MessageAuthor as DomainAuthor, MessagePart as DomainPart,
    MessageStatus as DomainStatus, QueuedPrompt as DomainPrompt,
    QueuedPromptKind as DomainPromptKind,
};
use homebot_protocol::{
    ActivityDetail, ActivityKind, ActivityPresentation, ActivityStatus, ActivitySummary,
    ApprovalDecisionRequest, ApprovalStatus, ApprovalSummary, Attachment, BotMutationRequest,
    ChatSummary, ChatTimelineResponse, CreateDirectChatRequest, CreateDirectChatResponse,
    MessageAuthor, MessagePart, MessageStatus, MessageSummary, QueuedPromptSummary,
    SendMessageRequest, SendMessageResponse, ServerEventBody,
};
use homebot_skills::AppliedSkill;
use homebot_storage::{IdempotencyClaim, QueuedPromptInput};
use uuid::Uuid;

use crate::{
    AppState,
    bots::{ApiError, claim},
    persist_event, unix_time_ms,
};

pub(super) async fn create_direct(
    State(state): State<AppState>,
    Json(request): Json<CreateDirectChatRequest>,
) -> Result<(StatusCode, Json<CreateDirectChatResponse>), ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            "create_direct_chat",
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let chat = state
        .storage
        .create_direct_chat(
            state.owner_id,
            request.bot_id,
            request.idempotency_key,
            unix_time_ms(),
        )
        .await?;
    let chat = chat_summary(chat);
    if !replayed {
        publish(
            &state,
            "chat_changed",
            ServerEventBody::ChatChanged { chat: chat.clone() },
        )
        .await?;
    }
    Ok((
        if replayed {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(CreateDirectChatResponse { chat }),
    ))
}

pub(super) async fn timeline(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
) -> Result<Json<ChatTimelineResponse>, ApiError> {
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    let messages = state.storage.chat_messages(state.owner_id, chat_id).await?;
    let prompts = state
        .storage
        .queued_prompts(state.owner_id, chat_id)
        .await?;
    let activities = state
        .storage
        .chat_activities(state.owner_id, chat_id)
        .await?;
    let approvals = state
        .storage
        .chat_approvals(state.owner_id, chat_id)
        .await?;
    let checkpoints = state
        .storage
        .turn_checkpoints(state.owner_id, chat_id)
        .await?
        .into_iter()
        .map(|checkpoint| crate::checkpoints::summary(&checkpoint))
        .collect();
    let mut summaries = Vec::with_capacity(messages.len());
    for message in messages {
        summaries.push(message_summary(&state, message).await?);
    }
    Ok(Json(ChatTimelineResponse {
        chat: chat_summary(chat),
        messages: summaries,
        activities: activities.into_iter().map(activity_summary).collect(),
        approvals: approvals.into_iter().map(approval_summary).collect(),
        queued_prompts: prompts.into_iter().map(prompt_summary).collect(),
        working_context: crate::working_context::summary(&state, chat_id).await?,
        checkpoints,
        boundary_sequence: state
            .storage
            .latest_sequence(state.owner_id)
            .await
            .unwrap_or(0),
    }))
}

#[allow(clippy::too_many_lines)]
pub(super) async fn send_message(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("send_message:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    let applied_skills = if replayed {
        Vec::new()
    } else {
        state
            .storage
            .resolve_applied_skills(state.owner_id, chat.bot_id, &request.skill_ids)
            .await?
    };
    if chat.running {
        let prompt = if replayed {
            state
                .storage
                .queued_prompts(state.owner_id, chat_id)
                .await?
                .into_iter()
                .find(|prompt| prompt.id == request.idempotency_key)
                .ok_or_else(ApiError::internal)?
        } else {
            state
                .storage
                .enqueue_prompt(
                    state.owner_id,
                    chat_id,
                    request.idempotency_key,
                    QueuedPromptInput {
                        content: &request.content,
                        attachment_ids: &request.attachment_ids,
                        applied_skills: &applied_skills,
                        kind: DomainPromptKind::FollowUp,
                    },
                    unix_time_ms(),
                )
                .await?
        };
        let prompt = prompt_summary(prompt);
        if !replayed {
            publish_queue_state(&state, chat_id).await?;
        }
        return Ok(Json(SendMessageResponse::Queued { prompt }));
    }

    let provider_prompt = prompt_with_skills(&request.content, &applied_skills)?;
    let provider_attachments = request.attachment_ids.clone();

    let message = if replayed {
        state
            .storage
            .chat_messages(state.owner_id, chat_id)
            .await?
            .into_iter()
            .find(|message| message.id == request.idempotency_key)
            .ok_or_else(ApiError::internal)?
    } else {
        state
            .storage
            .append_user_message(
                state.owner_id,
                chat_id,
                request.idempotency_key,
                &request.content,
                &request.attachment_ids,
                request.reply_to_message_id,
                request.mentioned_bot_ids,
                &applied_skills,
                unix_time_ms(),
            )
            .await?
    };
    let message = message_summary(&state, message).await?;
    if !replayed {
        publish(
            &state,
            "message_changed",
            ServerEventBody::MessageChanged {
                message: message.clone(),
            },
        )
        .await?;
    }
    if !replayed {
        let _ = crate::provider_turn::start_if_configured(
            &state,
            &chat,
            &provider_prompt,
            &provider_attachments,
        )
        .await?;
    }
    Ok(Json(SendMessageResponse::Sent { message }))
}

pub(super) async fn steer(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("steer_chat:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    if !chat.running {
        return Err(ApiError::conflict(
            "Steering is available only while this Bot is working",
        ));
    }
    let applied_skills = if replayed {
        Vec::new()
    } else {
        state
            .storage
            .resolve_applied_skills(state.owner_id, chat.bot_id, &request.skill_ids)
            .await?
    };
    let prompt = if replayed {
        state
            .storage
            .queued_prompts(state.owner_id, chat_id)
            .await?
            .into_iter()
            .find(|prompt| prompt.id == request.idempotency_key)
            .ok_or_else(ApiError::internal)?
    } else {
        state
            .storage
            .enqueue_prompt(
                state.owner_id,
                chat_id,
                request.idempotency_key,
                QueuedPromptInput {
                    content: &request.content,
                    attachment_ids: &request.attachment_ids,
                    applied_skills: &applied_skills,
                    kind: DomainPromptKind::Steering,
                },
                unix_time_ms(),
            )
            .await?
    };
    let prompt = prompt_summary(prompt);
    if !replayed {
        publish_queue_state(&state, chat_id).await?;
    }
    Ok(Json(SendMessageResponse::Queued { prompt }))
}

pub(super) async fn stop(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<ChatSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("stop_chat:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if !replayed {
        crate::provider_turn::cancel(&state, chat_id).await?;
    }
    let chat = if replayed {
        state
            .storage
            .get_direct_chat(state.owner_id, chat_id)
            .await?
    } else {
        state
            .storage
            .set_chat_running(state.owner_id, chat_id, false, unix_time_ms())
            .await?
    };
    let chat = chat_summary(chat);
    if !replayed {
        publish(
            &state,
            "chat_changed",
            ServerEventBody::ChatChanged { chat: chat.clone() },
        )
        .await?;
    }
    Ok(Json(chat))
}

pub(super) async fn mark_read(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<ChatSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("mark_chat_read:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let chat = if replayed {
        state
            .storage
            .get_direct_chat(state.owner_id, chat_id)
            .await?
    } else {
        state
            .storage
            .mark_chat_read(state.owner_id, chat_id, unix_time_ms())
            .await?
    };
    let chat = chat_summary(chat);
    if !replayed {
        publish(
            &state,
            "chat_changed",
            ServerEventBody::ChatChanged { chat: chat.clone() },
        )
        .await?;
        let bot = state.storage.get_bot(state.owner_id, chat.bot_id).await?;
        publish(
            &state,
            "bot_changed",
            ServerEventBody::BotChanged {
                bot: crate::bots::summary(&state, bot).await,
            },
        )
        .await?;
    }
    Ok(Json(chat))
}

pub(super) async fn retry(
    State(state): State<AppState>,
    Path((chat_id, message_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<ChatSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("retry_message:{message_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    if replayed {
        return Ok(Json(chat_summary(chat)));
    }
    if chat.running {
        return Err(ApiError::conflict("This Bot is already working"));
    }
    let messages = state.storage.chat_messages(state.owner_id, chat_id).await?;
    let failed_index = messages
        .iter()
        .position(|message| message.id == message_id && message.status == DomainStatus::Failed)
        .ok_or_else(|| ApiError::conflict("Only failed Bot messages can be retried"))?;
    let source = messages[..failed_index]
        .iter()
        .rev()
        .find(|message| message.author == DomainAuthor::User)
        .ok_or_else(ApiError::internal)?;
    let mut prompt = String::new();
    let mut attachments = Vec::new();
    for part in &source.parts {
        match part {
            DomainPart::Text { text, .. } => prompt.push_str(text),
            DomainPart::Attachment { attachment_id, .. } => attachments.push(*attachment_id),
            DomainPart::Notice { .. } => {}
        }
    }
    let applied_skills = state
        .storage
        .message_applied_skills(state.owner_id, source.id)
        .await?;
    let prompt = prompt_with_skills(&prompt, &applied_skills)?;
    if !crate::provider_turn::start_if_configured(&state, &chat, &prompt, &attachments).await? {
        return Err(ApiError::conflict(
            "Configure an available provider before retrying",
        ));
    }
    Ok(Json(chat_summary(
        state
            .storage
            .get_direct_chat(state.owner_id, chat_id)
            .await?,
    )))
}

pub(super) fn prompt_with_skills(
    content: &str,
    skills: &[AppliedSkill],
) -> Result<String, ApiError> {
    if skills.is_empty() {
        return Ok(content.to_owned());
    }
    let instructions = homebot_skills::assemble(skills)
        .map_err(|error| ApiError::validation(&error.to_string()))?;
    Ok(format!(
        "<homebot_skills>\n{instructions}</homebot_skills>\n\n<user_message>\n{content}\n</user_message>"
    ))
}

pub(super) async fn decide_approval(
    State(state): State<AppState>,
    Path(approval_id): Path<Uuid>,
    Json(request): Json<ApprovalDecisionRequest>,
) -> Result<Json<ApprovalSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("decide_approval:{approval_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if !replayed {
        crate::provider_turn::resolve_approval(&state, approval_id, request.allow).await?;
    }
    let approval = if replayed {
        state
            .storage
            .chat_approval(state.owner_id, approval_id)
            .await?
    } else {
        state
            .storage
            .decide_chat_approval(state.owner_id, approval_id, request.allow, unix_time_ms())
            .await?
    };
    let approval = approval_summary(approval);
    if !replayed {
        publish(
            &state,
            "approval_changed",
            ServerEventBody::ApprovalChanged {
                approval: approval.clone(),
            },
        )
        .await?;
    }
    Ok(Json(approval))
}

pub(super) async fn publish(
    state: &AppState,
    kind: &str,
    body: ServerEventBody,
) -> Result<(), ApiError> {
    persist_event(state, kind, body)
        .await
        .map(|_| ())
        .map_err(|()| ApiError::internal())
}

pub(super) fn chat_summary(chat: DomainChat) -> ChatSummary {
    ChatSummary {
        id: chat.id,
        title: chat.title,
        bot_id: chat.bot_id,
        unread_count: chat.unread_count,
        running: chat.running,
        queued_count: chat.queued_count,
        last_sequence: chat.last_sequence,
    }
}

pub(super) async fn message_summary(
    state: &AppState,
    message: DomainMessage,
) -> Result<MessageSummary, ApiError> {
    let applied_skills = state
        .storage
        .message_applied_skills(state.owner_id, message.id)
        .await?
        .into_iter()
        .map(|skill| homebot_protocol::AppliedSkillSummary {
            skill_id: skill.skill_id,
            skill_version_id: skill.version_id,
            name: skill.name,
            version: skill.version,
        })
        .collect();
    let mut parts = Vec::with_capacity(message.parts.len());
    for part in message.parts {
        parts.push(match part {
            DomainPart::Text { id, ordinal, text } => MessagePart::Text { id, ordinal, text },
            DomainPart::Notice { id, ordinal, text } => MessagePart::Notice { id, ordinal, text },
            DomainPart::Attachment {
                id,
                ordinal,
                attachment_id,
            } => {
                let record = state
                    .storage
                    .attachment(attachment_id, state.owner_id)
                    .await?
                    .ok_or(homebot_storage::StorageError::AttachmentUnavailable)?;
                MessagePart::Attachment {
                    id,
                    ordinal,
                    attachment: Attachment {
                        id: record.id,
                        filename: record.filename,
                        media_type: record.media_type,
                        size_bytes: record.size_bytes,
                        sha256: record.sha256,
                    },
                }
            }
        });
    }
    Ok(MessageSummary {
        id: message.id,
        chat_id: message.chat_id,
        author: match message.author {
            DomainAuthor::User => MessageAuthor::User,
            DomainAuthor::Bot => MessageAuthor::Bot,
            DomainAuthor::System => MessageAuthor::System,
        },
        author_bot_id: message.author_bot_id,
        status: match message.status {
            DomainStatus::Queued => MessageStatus::Queued,
            DomainStatus::Streaming => MessageStatus::Streaming,
            DomainStatus::Completed => MessageStatus::Completed,
            DomainStatus::Failed => MessageStatus::Failed,
            DomainStatus::Cancelled => MessageStatus::Cancelled,
        },
        parts,
        reply_to_message_id: message.reply_to_message_id,
        mentioned_bot_ids: message.mentioned_bot_ids,
        shared_context_message_ids: message.shared_context_message_ids,
        applied_skills,
        created_at_ms: message.created_at_ms,
        completed_at_ms: message.completed_at_ms,
        error: message
            .error_json
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| ApiError::internal())?,
    })
}

pub(super) fn prompt_summary(prompt: DomainPrompt) -> QueuedPromptSummary {
    QueuedPromptSummary {
        id: prompt.id,
        chat_id: prompt.chat_id,
        content: prompt.content,
        attachment_ids: prompt.attachment_ids,
        skill_ids: prompt.skill_ids,
        kind: match prompt.kind {
            DomainPromptKind::FollowUp => homebot_protocol::QueuedPromptKind::FollowUp,
            DomainPromptKind::Steering => homebot_protocol::QueuedPromptKind::Steering,
        },
        position: prompt.position,
        created_at_ms: prompt.created_at_ms,
    }
}

async fn publish_queue_state(state: &AppState, chat_id: Uuid) -> Result<(), ApiError> {
    for prompt in state
        .storage
        .queued_prompts(state.owner_id, chat_id)
        .await?
    {
        publish(
            state,
            "queued_prompt_changed",
            ServerEventBody::QueuedPromptChanged {
                prompt: prompt_summary(prompt),
            },
        )
        .await?;
    }
    let chat = state
        .storage
        .get_direct_chat(state.owner_id, chat_id)
        .await?;
    publish(
        state,
        "chat_changed",
        ServerEventBody::ChatChanged {
            chat: chat_summary(chat),
        },
    )
    .await
}

pub(super) fn activity_summary(activity: DomainActivity) -> ActivitySummary {
    let presentation = serde_json::from_value(activity.presentation_json)
        .ok()
        .filter(ActivityPresentation::is_remote_safe)
        .unwrap_or_else(|| ActivityPresentation {
            risk: homebot_protocol::RiskLevel::Low,
            detail: ActivityDetail::Generic {
                summary: activity.detail.clone(),
            },
            copy_text: None,
            open_artifact_id: None,
        });

    ActivitySummary {
        id: activity.id,
        chat_id: activity.chat_id,
        message_id: activity.message_id,
        title: activity.title,
        detail: activity.detail,
        kind: match activity.kind.as_str() {
            "reasoning" => ActivityKind::Reasoning,
            "search" => ActivityKind::Search,
            "filesystem" => ActivityKind::Filesystem,
            "terminal" => ActivityKind::Terminal,
            "browser" => ActivityKind::Browser,
            "artifact" => ActivityKind::Artifact,
            _ => ActivityKind::Tool,
        },
        presentation,
        status: match activity.status {
            DomainActivityStatus::Pending => ActivityStatus::Pending,
            DomainActivityStatus::Running => ActivityStatus::Running,
            DomainActivityStatus::Succeeded => ActivityStatus::Succeeded,
            DomainActivityStatus::Failed => ActivityStatus::Failed,
            DomainActivityStatus::Cancelled => ActivityStatus::Cancelled,
        },
        requires_attention: activity.requires_attention,
        started_at_ms: activity.started_at_ms,
        finished_at_ms: activity.finished_at_ms,
    }
}

pub(super) fn approval_summary(approval: DomainApproval) -> ApprovalSummary {
    ApprovalSummary {
        id: approval.id,
        chat_id: approval.chat_id,
        message_id: approval.message_id,
        title: approval.title,
        detail: approval.detail,
        status: match approval.status {
            DomainApprovalStatus::Pending => ApprovalStatus::Pending,
            DomainApprovalStatus::Allowed => ApprovalStatus::Allowed,
            DomainApprovalStatus::Denied => ApprovalStatus::Denied,
            DomainApprovalStatus::Expired => ApprovalStatus::Expired,
        },
        created_at_ms: approval.created_at_ms,
        decided_at_ms: approval.decided_at_ms,
    }
}
