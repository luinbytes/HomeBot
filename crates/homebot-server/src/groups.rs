//! Server-authoritative multi-Bot group chat and coordination APIs.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use homebot_domain::chat::{
    GroupBotStatus as DomainBotStatus, GroupChat, GroupParticipant,
    GroupParticipantRole as DomainRole, MessagePart as DomainMessagePart, OwnershipHandoff,
};
use homebot_protocol::{
    AddGroupParticipantRequest, BotMutationRequest, CreateGroupChatRequest,
    CreateGroupChatResponse, GroupBotStatus, GroupChatSummary, GroupParticipantRole,
    GroupParticipantSummary, GroupTimelineResponse, HandoffGroupRequest, MessageSummary,
    OwnershipHandoffSummary, RenameGroupChatRequest, SendGroupMessageRequest, ServerEventBody,
    UpdateGroupParticipantRequest,
};
use homebot_providers::{ProviderTool, ProviderToolCall, ProviderToolResult};
use homebot_storage::IdempotencyClaim;
use serde_json::json;
use uuid::Uuid;

use crate::{
    AppState,
    bots::{ApiError, claim},
    chats::{message_summary, publish, storage_references},
    unix_time_ms,
};

const HANDOFF_TOOL: &str = "homebot_handoff";

pub(super) async fn provider_tools(
    state: &AppState,
    chat_id: Uuid,
    from_bot_id: Uuid,
) -> Result<Vec<ProviderTool>, ApiError> {
    let participants = state
        .storage
        .group_participants(state.owner_id, chat_id)
        .await?;
    let mut recipients = Vec::new();
    for participant in participants {
        if participant.bot_id != from_bot_id {
            let bot = state
                .storage
                .get_bot(state.owner_id, participant.bot_id)
                .await?;
            recipients.push(json!({
                "const": participant.bot_id.to_string(),
                "title": bot.name,
            }));
        }
    }
    if recipients.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![ProviderTool {
        name: HANDOFF_TOOL.to_owned(),
        description: "Hand your current findings or work to another Bot in this group. Write the useful findings in your response before calling this tool; HomeBot persists that message, starts the recipient independently, and shows the handoff to the user.".to_owned(),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "to_bot_id": {
                    "type": "string",
                    "oneOf": recipients,
                    "description": "The HomeBot teammate that should continue this work"
                },
                "reason": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 1000,
                    "description": "What the recipient should do with your findings"
                }
            },
            "required": ["to_bot_id", "reason"]
        }),
    }])
}

pub(super) async fn handle_provider_tool(
    state: &AppState,
    chat_id: Uuid,
    from_bot_id: Uuid,
    message_id: Uuid,
    call: &ProviderToolCall,
) -> ProviderToolResult {
    let result = provider_handoff(state, chat_id, from_bot_id, message_id, call).await;
    match result {
        Ok(content) => ProviderToolResult {
            success: true,
            content,
        },
        Err(error) => ProviderToolResult {
            success: false,
            content: error,
        },
    }
}

async fn provider_handoff(
    state: &AppState,
    chat_id: Uuid,
    from_bot_id: Uuid,
    message_id: Uuid,
    call: &ProviderToolCall,
) -> Result<String, String> {
    if call.name != HANDOFF_TOOL {
        return Err("HomeBot does not recognize this collaboration tool".to_owned());
    }
    let to_bot_id = call
        .arguments
        .get("to_bot_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| "Choose a valid Bot from the handoff tool options".to_owned())?;
    let reason = call
        .arguments
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 1000)
        .ok_or_else(|| "Explain the handoff in 1 to 1000 characters".to_owned())?;
    let group = state
        .storage
        .get_group_chat(state.owner_id, chat_id)
        .await
        .map_err(|_| "This group chat is no longer available".to_owned())?;
    let participants = state
        .storage
        .group_participants(state.owner_id, chat_id)
        .await
        .map_err(|_| "HomeBot could not read the group participants".to_owned())?;
    if to_bot_id == from_bot_id
        || !participants
            .iter()
            .any(|participant| participant.bot_id == from_bot_id)
        || !participants
            .iter()
            .any(|participant| participant.bot_id == to_bot_id)
    {
        return Err("The handoff recipient must be another Bot in this group".to_owned());
    }
    let running = participants
        .iter()
        .filter(|participant| participant.status == DomainBotStatus::Running)
        .count();
    let recipient_running = participants.iter().any(|participant| {
        participant.bot_id == to_bot_id && participant.status == DomainBotStatus::Running
    });
    if group.stop_requested
        || group.coordination_turns_used >= group.coordination_max_turns
        || running >= usize::try_from(group.max_parallel_bots).unwrap_or(usize::MAX)
        || recipient_running
    {
        return Err("The handoff cannot start now because the group is stopped, busy, or out of coordination turns".to_owned());
    }
    let handoff = state
        .storage
        .handoff_group_ownership(
            state.owner_id,
            chat_id,
            Uuid::now_v7(),
            from_bot_id,
            to_bot_id,
            Some(message_id),
            reason,
            unix_time_ms(),
        )
        .await
        .map_err(|_| "HomeBot could not persist the handoff".to_owned())?;
    publish_recorded_handoff(state, &handoff)
        .await
        .map_err(|_| "HomeBot persisted the handoff but could not publish it".to_owned())?;
    start_handoff_turn(
        state,
        chat_id,
        from_bot_id,
        to_bot_id,
        Some(message_id),
        reason,
    )
    .await
    .map_err(|_| "HomeBot persisted the handoff but could not start the recipient".to_owned())?;
    let recipient = state
        .storage
        .get_bot(state.owner_id, to_bot_id)
        .await
        .map_err(|_| "The receiving Bot is no longer available".to_owned())?;
    Ok(format!(
        "Handoff recorded. {} is now working on it.",
        recipient.name
    ))
}

pub(super) async fn create(
    State(state): State<AppState>,
    Json(request): Json<CreateGroupChatRequest>,
) -> Result<(StatusCode, Json<CreateGroupChatResponse>), ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            "create_group_chat",
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let group = if replayed {
        state
            .storage
            .get_group_chat(state.owner_id, request.idempotency_key)
            .await?
    } else {
        state
            .storage
            .create_group_chat(
                state.owner_id,
                request.idempotency_key,
                &request.title,
                &request.bot_ids,
                request.ownership_bot_id,
                request.coordination_max_turns,
                request.max_parallel_bots,
                unix_time_ms(),
            )
            .await?
    };
    let participants = state
        .storage
        .group_participants(state.owner_id, group.id)
        .await?;
    let group = group_summary(group);
    if !replayed {
        publish(
            &state,
            "group_chat_changed",
            ServerEventBody::GroupChatChanged {
                group: group.clone(),
            },
        )
        .await?;
        for participant in &participants {
            publish(
                &state,
                "group_participant_changed",
                ServerEventBody::GroupParticipantChanged {
                    participant: participant_summary(participant),
                },
            )
            .await?;
        }
    }
    Ok((
        if replayed {
            StatusCode::OK
        } else {
            StatusCode::CREATED
        },
        Json(CreateGroupChatResponse {
            group,
            participants: participants.iter().map(participant_summary).collect(),
        }),
    ))
}

pub(super) async fn rename(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<RenameGroupChatRequest>,
) -> Result<Json<GroupChatSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("rename_group:{chat_id}"),
            &request
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let group = if replayed {
        state
            .storage
            .get_group_chat(state.owner_id, chat_id)
            .await?
    } else {
        state
            .storage
            .rename_group_chat(state.owner_id, chat_id, &request.title, unix_time_ms())
            .await?
    };
    let group = group_summary(group);
    if !replayed {
        publish(
            &state,
            "group_chat_changed",
            ServerEventBody::GroupChatChanged {
                group: group.clone(),
            },
        )
        .await?;
    }
    Ok(Json(group))
}

pub(super) async fn timeline(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
) -> Result<Json<GroupTimelineResponse>, ApiError> {
    let group = state
        .storage
        .get_group_chat(state.owner_id, chat_id)
        .await?;
    let participants = state
        .storage
        .group_participants(state.owner_id, chat_id)
        .await?;
    let messages = state.storage.chat_messages(state.owner_id, chat_id).await?;
    let mut message_summaries = Vec::with_capacity(messages.len());
    for message in messages {
        message_summaries.push(message_summary(&state, message).await?);
    }
    Ok(Json(GroupTimelineResponse {
        group: group_summary(group),
        participants: participants.iter().map(participant_summary).collect(),
        messages: message_summaries,
        handoffs: state
            .storage
            .group_handoffs(state.owner_id, chat_id)
            .await?
            .into_iter()
            .map(handoff_summary)
            .collect(),
        boundary_sequence: state
            .storage
            .latest_sequence(state.owner_id)
            .await
            .unwrap_or(0),
    }))
}

pub(super) async fn send_message(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<SendGroupMessageRequest>,
) -> Result<Json<MessageSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("send_group_message:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let group = state
        .storage
        .get_group_chat(state.owner_id, chat_id)
        .await?;
    let mut target_bot_ids = if request.mentioned_bot_ids.is_empty() {
        vec![group.ownership_bot_id]
    } else {
        Vec::with_capacity(request.mentioned_bot_ids.len())
    };
    for bot_id in &request.mentioned_bot_ids {
        if !target_bot_ids.contains(bot_id) {
            target_bot_ids.push(*bot_id);
        }
    }
    let requested_turns = u32::try_from(target_bot_ids.len()).unwrap_or(u32::MAX);
    let participants = state
        .storage
        .group_participants(state.owner_id, chat_id)
        .await?;
    let running = u32::try_from(
        participants
            .iter()
            .filter(|participant| participant.status == DomainBotStatus::Running)
            .count(),
    )
    .unwrap_or(u32::MAX);
    let target_running = participants.iter().any(|participant| {
        target_bot_ids.contains(&participant.bot_id)
            && participant.status == DomainBotStatus::Running
    });
    if !replayed
        && (group.stop_requested
            || target_running
            || running.saturating_add(requested_turns) > group.max_parallel_bots
            || requested_turns
                > group
                    .coordination_max_turns
                    .saturating_sub(group.coordination_turns_used))
    {
        return Err(homebot_storage::StorageError::CoordinationLimitReached.into());
    }
    let message = if replayed {
        state
            .storage
            .message(state.owner_id, request.idempotency_key)
            .await?
    } else {
        state
            .storage
            .append_group_user_message(
                state.owner_id,
                chat_id,
                request.idempotency_key,
                &request.content,
                &request.mentioned_bot_ids,
                &request.shared_context_message_ids,
                request.reply_to_message_id,
                &storage_references(&request.references),
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
        for bot_id in target_bot_ids {
            let _ = crate::provider_turn::start_group_if_configured(
                &state,
                chat_id,
                bot_id,
                &request.content,
            )
            .await?;
        }
    }
    Ok(Json(message))
}

#[allow(clippy::too_many_lines)]
pub(super) async fn handoff(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<HandoffGroupRequest>,
) -> Result<Json<OwnershipHandoffSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("handoff_group:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let should_start = if replayed {
        false
    } else {
        let group = state
            .storage
            .get_group_chat(state.owner_id, chat_id)
            .await?;
        let participants = state
            .storage
            .group_participants(state.owner_id, chat_id)
            .await?;
        let running = participants
            .iter()
            .filter(|participant| participant.status == DomainBotStatus::Running)
            .count();
        let recipient_running = participants.iter().any(|participant| {
            participant.bot_id == request.to_bot_id
                && participant.status == DomainBotStatus::Running
        });
        !group.stop_requested
            && group.coordination_turns_used < group.coordination_max_turns
            && running < usize::try_from(group.max_parallel_bots).unwrap_or(usize::MAX)
            && !recipient_running
    };
    let handoff = if replayed {
        state
            .storage
            .group_handoffs(state.owner_id, chat_id)
            .await?
            .into_iter()
            .find(|handoff| handoff.id == request.idempotency_key)
            .ok_or_else(ApiError::internal)?
    } else {
        state
            .storage
            .handoff_group_ownership(
                state.owner_id,
                chat_id,
                request.idempotency_key,
                request.from_bot_id,
                request.to_bot_id,
                request.message_id,
                &request.reason,
                unix_time_ms(),
            )
            .await?
    };
    if !replayed {
        publish_recorded_handoff(&state, &handoff).await?;
        if should_start {
            start_handoff_turn(
                &state,
                chat_id,
                request.from_bot_id,
                request.to_bot_id,
                request.message_id,
                &request.reason,
            )
            .await?;
        }
    }
    Ok(Json(handoff_summary(handoff)))
}

async fn publish_recorded_handoff(
    state: &AppState,
    handoff: &OwnershipHandoff,
) -> Result<(), ApiError> {
    publish(
        state,
        "group_handoff_recorded",
        ServerEventBody::GroupHandoffRecorded {
            handoff: handoff_summary(handoff.clone()),
        },
    )
    .await?;
    let group = state
        .storage
        .get_group_chat(state.owner_id, handoff.chat_id)
        .await?;
    publish(
        state,
        "group_chat_changed",
        ServerEventBody::GroupChatChanged {
            group: group_summary(group),
        },
    )
    .await
}

async fn start_handoff_turn(
    state: &AppState,
    chat_id: Uuid,
    from_bot_id: Uuid,
    to_bot_id: Uuid,
    message_id: Option<Uuid>,
    reason: &str,
) -> Result<(), ApiError> {
    let from_bot = state.storage.get_bot(state.owner_id, from_bot_id).await?;
    let shared_message = if let Some(message_id) = message_id {
        let message = state.storage.message(state.owner_id, message_id).await?;
        message
            .parts
            .iter()
            .filter_map(|part| match part {
                DomainMessagePart::Text { text, .. } | DomainMessagePart::Notice { text, .. } => {
                    Some(text.as_str())
                }
                DomainMessagePart::Attachment { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };
    let prompt = format!(
        "<homebot_handoff>\nFrom: {}\nReason: {}\nShared message:\n{}\n</homebot_handoff>\n\nContinue the work from this handoff.",
        from_bot.name, reason, shared_message
    );
    let _ =
        crate::provider_turn::start_group_if_configured(state, chat_id, to_bot_id, &prompt).await?;
    Ok(())
}

pub(super) async fn update_participant(
    State(state): State<AppState>,
    Path((chat_id, bot_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<UpdateGroupParticipantRequest>,
) -> Result<Json<GroupParticipantSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("group_participant_status:{chat_id}:{bot_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let participant = if replayed {
        state
            .storage
            .group_participants(state.owner_id, chat_id)
            .await?
            .into_iter()
            .find(|participant| participant.bot_id == bot_id)
            .ok_or(homebot_storage::StorageError::InvalidGroupParticipants)?
    } else {
        state
            .storage
            .set_group_bot_status(
                state.owner_id,
                chat_id,
                bot_id,
                to_domain_status(request.status),
                request.operation_id,
                unix_time_ms(),
            )
            .await?
    };
    let participant = participant_summary(&participant);
    if !replayed {
        publish(
            &state,
            "group_participant_changed",
            ServerEventBody::GroupParticipantChanged {
                participant: participant.clone(),
            },
        )
        .await?;
    }
    Ok(Json(participant))
}

pub(super) async fn add_participant(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<AddGroupParticipantRequest>,
) -> Result<Json<GroupParticipantSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("add_group_participant:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let participant = if replayed {
        state
            .storage
            .group_participants(state.owner_id, chat_id)
            .await?
            .into_iter()
            .find(|participant| participant.bot_id == request.bot_id)
            .ok_or(homebot_storage::StorageError::InvalidGroupParticipants)?
    } else {
        state
            .storage
            .add_group_participant(state.owner_id, chat_id, request.bot_id, unix_time_ms())
            .await?
    };
    let participant = participant_summary(&participant);
    if !replayed {
        publish(
            &state,
            "group_participant_changed",
            ServerEventBody::GroupParticipantChanged {
                participant: participant.clone(),
            },
        )
        .await?;
    }
    Ok(Json(participant))
}

pub(super) async fn remove_participant(
    State(state): State<AppState>,
    Path((chat_id, bot_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<BotMutationRequest>,
) -> Result<StatusCode, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("remove_group_participant:{chat_id}:{bot_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    if !replayed {
        state
            .storage
            .remove_group_participant(state.owner_id, chat_id, bot_id)
            .await?;
        publish(
            &state,
            "group_participant_removed",
            ServerEventBody::GroupParticipantRemoved { chat_id, bot_id },
        )
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn record_turn(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<GroupChatSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("group_coordination_turn:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let group = if replayed {
        state
            .storage
            .get_group_chat(state.owner_id, chat_id)
            .await?
    } else {
        state
            .storage
            .record_group_coordination_turn(state.owner_id, chat_id, unix_time_ms())
            .await?
    };
    let group = group_summary(group);
    if !replayed {
        publish(
            &state,
            "group_chat_changed",
            ServerEventBody::GroupChatChanged {
                group: group.clone(),
            },
        )
        .await?;
    }
    Ok(Json(group))
}

pub(super) async fn stop(
    State(state): State<AppState>,
    Path(chat_id): Path<Uuid>,
    Json(request): Json<BotMutationRequest>,
) -> Result<Json<GroupChatSummary>, ApiError> {
    let replayed = matches!(
        claim(
            &state,
            request.idempotency_key,
            &format!("stop_group:{chat_id}"),
            &request,
        )
        .await?,
        IdempotencyClaim::Replayed { .. }
    );
    let group = if replayed {
        state
            .storage
            .get_group_chat(state.owner_id, chat_id)
            .await?
    } else {
        crate::provider_turn::cancel_group(&state, chat_id).await?;
        state
            .storage
            .stop_group_chat(state.owner_id, chat_id, unix_time_ms())
            .await?
    };
    let group = group_summary(group);
    if !replayed {
        publish(
            &state,
            "group_chat_changed",
            ServerEventBody::GroupChatChanged {
                group: group.clone(),
            },
        )
        .await?;
    }
    Ok(Json(group))
}

pub(super) fn group_summary(group: GroupChat) -> GroupChatSummary {
    GroupChatSummary {
        id: group.id,
        title: group.title,
        ownership_bot_id: group.ownership_bot_id,
        coordination_max_turns: group.coordination_max_turns,
        coordination_turns_used: group.coordination_turns_used,
        max_parallel_bots: group.max_parallel_bots,
        stop_requested: group.stop_requested,
    }
}

pub(super) fn participant_summary(participant: &GroupParticipant) -> GroupParticipantSummary {
    GroupParticipantSummary {
        chat_id: participant.chat_id,
        bot_id: participant.bot_id,
        role: match participant.role {
            DomainRole::Owner => GroupParticipantRole::Owner,
            DomainRole::Member => GroupParticipantRole::Member,
        },
        status: match participant.status {
            DomainBotStatus::Idle => GroupBotStatus::Idle,
            DomainBotStatus::Running => GroupBotStatus::Running,
            DomainBotStatus::Waiting => GroupBotStatus::Waiting,
            DomainBotStatus::Completed => GroupBotStatus::Completed,
            DomainBotStatus::Failed => GroupBotStatus::Failed,
            DomainBotStatus::Stopped => GroupBotStatus::Stopped,
        },
        active_operation_id: participant.active_operation_id,
        updated_at_ms: participant.updated_at_ms,
    }
}

fn handoff_summary(handoff: OwnershipHandoff) -> OwnershipHandoffSummary {
    OwnershipHandoffSummary {
        id: handoff.id,
        chat_id: handoff.chat_id,
        from_bot_id: handoff.from_bot_id,
        to_bot_id: handoff.to_bot_id,
        message_id: handoff.message_id,
        reason: handoff.reason,
        created_at_ms: handoff.created_at_ms,
    }
}

fn to_domain_status(status: GroupBotStatus) -> DomainBotStatus {
    match status {
        GroupBotStatus::Idle => DomainBotStatus::Idle,
        GroupBotStatus::Running => DomainBotStatus::Running,
        GroupBotStatus::Waiting => DomainBotStatus::Waiting,
        GroupBotStatus::Completed => DomainBotStatus::Completed,
        GroupBotStatus::Failed => DomainBotStatus::Failed,
        GroupBotStatus::Stopped => DomainBotStatus::Stopped,
    }
}
