use super::{
    ApprovalDecisionRequest, BotClientCommand, BotEditorDraft, BotMutationRequest, BotResponse,
    Client, ComposerDraft, CreateAttachmentRequest, CreateAttachmentResponse, CreateBotRequest,
    CreateDirectChatRequest, CreateDirectChatResponse, DeleteBotRequest, DesktopCommand,
    DesktopEvent, Digest, ErrorEnvelope, FinalizeAttachmentRequest, MessageMutationRequest, Method,
    ReactionMutationRequest, RuntimeConfig, SendMessageRequest, Sender, Sha256, StatusCode,
    TimelineCommand, TransportFailure, UpdateBotRequest, Uuid, WorkspaceCommand, protocol_error,
    request_error,
};

pub(super) async fn execute_command(
    client: &Client,
    config: &RuntimeConfig,
    command: DesktopCommand,
    events: &Sender<DesktopEvent>,
) -> Result<(), TransportFailure> {
    match command {
        DesktopCommand::Bot(command) => execute_bot(client, config, command, events).await,
        DesktopCommand::LoadTimeline(chat_id) => {
            let timeline = response_json(
                authenticated(
                    client,
                    config,
                    Method::GET,
                    &format!("/api/v1/chats/{chat_id}/timeline"),
                )
                .send()
                .await
                .map_err(request_error)?,
            )
            .await?;
            let _ = events.send(DesktopEvent::Timeline(timeline));
            Ok(())
        }
        DesktopCommand::Timeline {
            bot_id,
            chat_id,
            command,
        } => execute_timeline(client, config, bot_id, chat_id, command, events).await,
        DesktopCommand::UploadAttachment {
            filename,
            media_type,
            bytes,
        } => {
            let attachment = upload_attachment(client, config, filename, media_type, bytes).await?;
            let _ = events.send(DesktopEvent::AttachmentUploaded(attachment));
            Ok(())
        }
        DesktopCommand::Workspace(command) => {
            execute_workspace(client, config, command, events).await
        }
        DesktopCommand::LoadDevices => load_devices(client, config, events).await,
        DesktopCommand::CreatePairing {
            endpoint,
            allow_insecure_private_network,
        } => {
            create_pairing(
                client,
                config,
                events,
                endpoint,
                allow_insecure_private_network,
            )
            .await
        }
        DesktopCommand::RevokeDevice(device_id) => {
            revoke_device(client, config, events, device_id).await
        }
        DesktopCommand::Search(query) => {
            let response = response_json(
                authenticated(client, config, Method::GET, "/api/v1/search")
                    .query(&[("q", query)])
                    .send()
                    .await
                    .map_err(request_error)?,
            )
            .await?;
            let _ = events.send(DesktopEvent::Search(response));
            Ok(())
        }
        DesktopCommand::Shutdown => Ok(()),
    }
}

async fn load_devices(
    client: &Client,
    config: &RuntimeConfig,
    events: &Sender<DesktopEvent>,
) -> Result<(), TransportFailure> {
    let devices = response_json(
        authenticated(client, config, Method::GET, "/api/v1/devices")
            .send()
            .await
            .map_err(request_error)?,
    )
    .await?;
    let _ = events.send(DesktopEvent::Devices(devices));
    Ok(())
}

async fn create_pairing(
    client: &Client,
    config: &RuntimeConfig,
    events: &Sender<DesktopEvent>,
    endpoint: String,
    allow_insecure_private_network: bool,
) -> Result<(), TransportFailure> {
    let offer = response_json(
        authenticated(client, config, Method::POST, "/api/v1/pairing")
            .json(&super::CreatePairingRequest {
                request_id: Uuid::now_v7(),
                endpoint,
                allow_insecure_private_network,
            })
            .send()
            .await
            .map_err(request_error)?,
    )
    .await?;
    let _ = events.send(DesktopEvent::PairingOffer(offer));
    Ok(())
}

async fn revoke_device(
    client: &Client,
    config: &RuntimeConfig,
    events: &Sender<DesktopEvent>,
    device_id: Uuid,
) -> Result<(), TransportFailure> {
    let device = response_json(
        authenticated(
            client,
            config,
            Method::POST,
            &format!("/api/v1/devices/{device_id}/revoke"),
        )
        .json(&super::RevokeDeviceSessionRequest {
            request_id: Uuid::now_v7(),
            idempotency_key: Uuid::now_v7(),
        })
        .send()
        .await
        .map_err(request_error)?,
    )
    .await?;
    let _ = events.send(DesktopEvent::DeviceRevoked(device));
    Ok(())
}

async fn execute_workspace(
    client: &Client,
    config: &RuntimeConfig,
    command: WorkspaceCommand,
    events: &Sender<DesktopEvent>,
) -> Result<(), TransportFailure> {
    match command {
        WorkspaceCommand::RegisterRepository { root_path, name } => {
            let body = super::CreateRepositoryWorkspaceRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                root_path,
                name,
            };
            let workspace = response_json(
                authenticated(client, config, Method::POST, "/api/v1/workspaces")
                    .json(&body)
                    .send()
                    .await
                    .map_err(request_error)?,
            )
            .await?;
            let _ = events.send(DesktopEvent::RepositoryWorkspaceRegistered(workspace));
            Ok(())
        }
        WorkspaceCommand::Attach {
            chat_id,
            workspace_id,
            mode,
            base_ref,
            branch_name,
        } => {
            let body = super::AttachChatWorkspaceRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                workspace_id,
                mode,
                base_ref,
                branch_name,
            };
            let workspace = response_json(
                authenticated(
                    client,
                    config,
                    Method::PUT,
                    &format!("/api/v1/chats/{chat_id}/workspace"),
                )
                .json(&body)
                .send()
                .await
                .map_err(request_error)?,
            )
            .await?;
            let _ = events.send(DesktopEvent::ChatWorkspaceAttached(workspace));
            Ok(())
        }
        WorkspaceCommand::Detach { chat_id } => {
            let body = super::DetachChatWorkspaceRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            };
            ensure_success(
                authenticated(
                    client,
                    config,
                    Method::POST,
                    &format!("/api/v1/chats/{chat_id}/workspace/detach"),
                )
                .json(&body)
                .send()
                .await
                .map_err(request_error)?,
            )
            .await?;
            let _ = events.send(DesktopEvent::ChatWorkspaceDetached(chat_id));
            Ok(())
        }
        WorkspaceCommand::LoadBranches { workspace_id } => {
            let response: super::WorkspaceBranchesResponse = response_json(
                authenticated(
                    client,
                    config,
                    Method::GET,
                    &format!("/api/v1/workspaces/{workspace_id}/branches"),
                )
                .send()
                .await
                .map_err(request_error)?,
            )
            .await?;
            let _ = events.send(DesktopEvent::WorkspaceBranches {
                workspace_id,
                branches: response.branches,
            });
            Ok(())
        }
        command => execute_vcs(client, config, command, events).await,
    }
}

async fn execute_vcs(
    client: &Client,
    config: &RuntimeConfig,
    command: WorkspaceCommand,
    events: &Sender<DesktopEvent>,
) -> Result<(), TransportFailure> {
    match command {
        WorkspaceCommand::LoadStatus { chat_id } => {
            load_vcs_status(client, config, events, chat_id).await
        }
        WorkspaceCommand::LoadDiff { chat_id, staged } => {
            load_vcs_diff(client, config, events, chat_id, staged).await
        }
        WorkspaceCommand::Commit {
            chat_id,
            message,
            stage_all,
        } => commit_vcs(client, config, events, chat_id, message, stage_all).await,
        WorkspaceCommand::CreateBranch {
            chat_id,
            branch,
            start_point,
        } => create_vcs_branch(client, config, events, chat_id, branch, start_point).await,
        WorkspaceCommand::Push {
            chat_id,
            request_id,
            idempotency_key,
            remote,
            branch,
            set_upstream,
            approval_id,
        } => {
            push_vcs(
                client,
                config,
                events,
                chat_id,
                super::VcsPushRequest {
                    request_id,
                    idempotency_key,
                    remote,
                    branch,
                    set_upstream,
                    approval_id,
                },
            )
            .await
        }
        WorkspaceCommand::LoadPullRequest {
            chat_id,
            remote,
            head_branch,
            base_branch,
        } => {
            load_pull_request(
                client,
                config,
                events,
                chat_id,
                remote,
                head_branch,
                base_branch,
            )
            .await
        }
        WorkspaceCommand::CreatePullRequest {
            chat_id,
            request_id,
            idempotency_key,
            remote,
            head_branch,
            base_branch,
            title,
            body,
            draft,
            approval_id,
        } => {
            create_pull_request(
                client,
                config,
                events,
                chat_id,
                super::CreatePullRequestRequest {
                    request_id,
                    idempotency_key,
                    remote,
                    head_branch,
                    base_branch,
                    title,
                    body,
                    draft,
                    approval_id,
                },
            )
            .await
        }
        _ => unreachable!("repository workspace commands are handled before VCS dispatch"),
    }
}

async fn load_vcs_status(
    client: &Client,
    config: &RuntimeConfig,
    events: &Sender<DesktopEvent>,
    chat_id: Uuid,
) -> Result<(), TransportFailure> {
    let status = get_json(
        client,
        config,
        &format!("/api/v1/chats/{chat_id}/vcs/status"),
    )
    .await?;
    let _ = events.send(DesktopEvent::VcsStatus { chat_id, status });
    Ok(())
}

async fn load_vcs_diff(
    client: &Client,
    config: &RuntimeConfig,
    events: &Sender<DesktopEvent>,
    chat_id: Uuid,
    staged: bool,
) -> Result<(), TransportFailure> {
    let diff = get_json(
        client,
        config,
        &format!("/api/v1/chats/{chat_id}/vcs/diff?staged={staged}"),
    )
    .await?;
    let _ = events.send(DesktopEvent::VcsDiff { chat_id, diff });
    Ok(())
}

async fn commit_vcs(
    client: &Client,
    config: &RuntimeConfig,
    events: &Sender<DesktopEvent>,
    chat_id: Uuid,
    message: String,
    stage_all: bool,
) -> Result<(), TransportFailure> {
    let body = super::VcsCommitRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        message,
        stage_all,
    };
    let result = post_json(
        client,
        config,
        &format!("/api/v1/chats/{chat_id}/vcs/commit"),
        &body,
    )
    .await?;
    let _ = events.send(DesktopEvent::VcsCommit { chat_id, result });
    Ok(())
}

async fn create_vcs_branch(
    client: &Client,
    config: &RuntimeConfig,
    events: &Sender<DesktopEvent>,
    chat_id: Uuid,
    branch: String,
    start_point: Option<String>,
) -> Result<(), TransportFailure> {
    let body = super::VcsCreateBranchRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        branch,
        start_point,
    };
    let status = post_json(
        client,
        config,
        &format!("/api/v1/chats/{chat_id}/vcs/branches"),
        &body,
    )
    .await?;
    let _ = events.send(DesktopEvent::VcsStatus { chat_id, status });
    Ok(())
}

async fn push_vcs(
    client: &Client,
    config: &RuntimeConfig,
    events: &Sender<DesktopEvent>,
    chat_id: Uuid,
    body: super::VcsPushRequest,
) -> Result<(), TransportFailure> {
    let response = post_json(
        client,
        config,
        &format!("/api/v1/chats/{chat_id}/vcs/push"),
        &body,
    )
    .await?;
    let _ = events.send(DesktopEvent::VcsRemoteMutation { chat_id, response });
    Ok(())
}

async fn load_pull_request(
    client: &Client,
    config: &RuntimeConfig,
    events: &Sender<DesktopEvent>,
    chat_id: Uuid,
    remote: String,
    head_branch: String,
    base_branch: String,
) -> Result<(), TransportFailure> {
    let metadata = response_json(
        authenticated(
            client,
            config,
            Method::GET,
            &format!("/api/v1/chats/{chat_id}/vcs/pull-request"),
        )
        .query(&[
            ("remote", remote),
            ("head_branch", head_branch),
            ("base_branch", base_branch),
        ])
        .send()
        .await
        .map_err(request_error)?,
    )
    .await?;
    let _ = events.send(DesktopEvent::PullRequestMetadata { chat_id, metadata });
    Ok(())
}

async fn create_pull_request(
    client: &Client,
    config: &RuntimeConfig,
    events: &Sender<DesktopEvent>,
    chat_id: Uuid,
    body: super::CreatePullRequestRequest,
) -> Result<(), TransportFailure> {
    let response = post_json(
        client,
        config,
        &format!("/api/v1/chats/{chat_id}/vcs/pull-request"),
        &body,
    )
    .await?;
    let _ = events.send(DesktopEvent::PullRequestMutation { chat_id, response });
    Ok(())
}

async fn get_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    config: &RuntimeConfig,
    path: &str,
) -> Result<T, TransportFailure> {
    response_json(
        authenticated(client, config, Method::GET, path)
            .send()
            .await
            .map_err(request_error)?,
    )
    .await
}

async fn post_json<T: serde::de::DeserializeOwned, B: serde::Serialize>(
    client: &Client,
    config: &RuntimeConfig,
    path: &str,
    body: &B,
) -> Result<T, TransportFailure> {
    response_json(
        authenticated(client, config, Method::POST, path)
            .json(body)
            .send()
            .await
            .map_err(request_error)?,
    )
    .await
}

async fn execute_bot(
    client: &Client,
    config: &RuntimeConfig,
    command: BotClientCommand,
    events: &Sender<DesktopEvent>,
) -> Result<(), TransportFailure> {
    if let BotClientCommand::Delete {
        bot_id,
        confirm_name,
    } = &command
    {
        let response = authenticated(
            client,
            config,
            Method::DELETE,
            &format!("/api/v1/bots/{bot_id}"),
        )
        .json(&DeleteBotRequest {
            request_id: Uuid::now_v7(),
            idempotency_key: Uuid::now_v7(),
            confirm_name: confirm_name.clone(),
        })
        .send()
        .await
        .map_err(request_error)?;
        return ensure_success(response).await;
    }
    let (method, path, body) = match command {
        BotClientCommand::Create(draft) => (
            Method::POST,
            "/api/v1/bots".to_owned(),
            serde_json::to_value(create_bot(draft)).map_err(protocol_error)?,
        ),
        BotClientCommand::Update(draft) => {
            let bot_id = draft
                .bot_id
                .ok_or_else(|| TransportFailure::Protocol("Missing Bot ID".to_owned()))?;
            (
                Method::PUT,
                format!("/api/v1/bots/{bot_id}"),
                serde_json::to_value(update_bot(draft)).map_err(protocol_error)?,
            )
        }
        BotClientCommand::Archive(bot_id) => mutation(bot_id, "archive")?,
        BotClientCommand::Restore(bot_id) => mutation(bot_id, "restore")?,
        BotClientCommand::Pin(bot_id) => mutation(bot_id, "pin")?,
        BotClientCommand::Unpin(bot_id) => mutation(bot_id, "unpin")?,
        BotClientCommand::Hide(bot_id) => mutation(bot_id, "hide")?,
        BotClientCommand::Unhide(bot_id) => mutation(bot_id, "unhide")?,
        BotClientCommand::Duplicate(bot_id) => mutation(bot_id, "duplicate")?,
        BotClientCommand::Delete { .. } => unreachable!("delete handled above"),
        BotClientCommand::MarkRead(bot_id) => mutation(bot_id, "read")?,
        BotClientCommand::RetryConnection => return Ok(()),
    };
    let response = authenticated(client, config, method, &path)
        .json(&body)
        .send()
        .await
        .map_err(request_error)?;
    let response: BotResponse = response_json(response).await?;
    let _ = events.send(DesktopEvent::BotMutation(response));
    Ok(())
}

fn mutation(
    bot_id: Uuid,
    action: &str,
) -> Result<(Method, String, serde_json::Value), TransportFailure> {
    let request = BotMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    Ok((
        Method::POST,
        format!("/api/v1/bots/{bot_id}/{action}"),
        serde_json::to_value(request).map_err(protocol_error)?,
    ))
}

fn create_bot(draft: BotEditorDraft) -> CreateBotRequest {
    CreateBotRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        name: draft.name,
        title: draft.title,
        description: draft.description,
        shape: draft.shape,
        color: draft.color,
        provider_profile_id: draft.provider_profile_id,
        permission_profile: draft.permission_profile,
    }
}

fn update_bot(draft: BotEditorDraft) -> UpdateBotRequest {
    UpdateBotRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        name: draft.name,
        title: draft.title,
        description: draft.description,
        shape: draft.shape,
        color: draft.color,
        provider_profile_id: draft.provider_profile_id,
        permission_profile: draft.permission_profile,
    }
}

async fn execute_timeline(
    client: &Client,
    config: &RuntimeConfig,
    bot_id: Uuid,
    chat_id: Option<Uuid>,
    command: TimelineCommand,
    events: &Sender<DesktopEvent>,
) -> Result<(), TransportFailure> {
    match command {
        TimelineCommand::Send(draft) | TimelineCommand::Queue(draft) => {
            let chat_id = match chat_id {
                Some(chat_id) => chat_id,
                None => create_direct_chat(client, config, bot_id).await?,
            };
            post_message(client, config, chat_id, draft, false).await
        }
        TimelineCommand::Steer(draft) => {
            let chat_id =
                chat_id.ok_or_else(|| TransportFailure::Protocol("No active chat".to_owned()))?;
            post_message(client, config, chat_id, draft, true).await
        }
        TimelineCommand::Stop => {
            let chat_id =
                chat_id.ok_or_else(|| TransportFailure::Protocol("No active chat".to_owned()))?;
            post_empty_mutation(client, config, &format!("/api/v1/chats/{chat_id}/stop")).await
        }
        TimelineCommand::Retry(message_id) => {
            retry_message(client, config, chat_id, message_id).await
        }
        TimelineCommand::SetReaction {
            message_id,
            emoji,
            active,
        } => set_reaction(client, config, message_id, emoji, active).await,
        TimelineCommand::DecideApproval { approval_id, allow } => {
            let body = ApprovalDecisionRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                allow,
            };
            ensure_success(
                authenticated(
                    client,
                    config,
                    Method::POST,
                    &format!("/api/v1/approvals/{approval_id}/decision"),
                )
                .json(&body)
                .send()
                .await
                .map_err(request_error)?,
            )
            .await
        }
        TimelineCommand::LoadCheckpointDiff {
            from_checkpoint_id,
            to_checkpoint_id,
        } => {
            let chat_id =
                chat_id.ok_or_else(|| TransportFailure::Protocol("No active chat".to_owned()))?;
            let diff = response_json(
                authenticated(
                    client,
                    config,
                    Method::GET,
                    &format!(
                        "/api/v1/chats/{chat_id}/checkpoints/diff?from_checkpoint_id={from_checkpoint_id}&to_checkpoint_id={to_checkpoint_id}"
                    ),
                )
                .send()
                .await
                .map_err(request_error)?,
            )
            .await?;
            let _ = events.send(DesktopEvent::CheckpointDiff(diff));
            Ok(())
        }
        TimelineCommand::RestoreCheckpoint(checkpoint_id) => {
            let body = super::RestoreCheckpointRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
            };
            ensure_success(
                authenticated(
                    client,
                    config,
                    Method::POST,
                    &format!("/api/v1/checkpoints/{checkpoint_id}/restore"),
                )
                .json(&body)
                .send()
                .await
                .map_err(request_error)?,
            )
            .await
        }
        command @ (TimelineCommand::SetInteractionMode(_)
        | TimelineCommand::CompactContext { .. }) => {
            execute_context_command(client, config, chat_id, command, events).await
        }
    }
}

async fn set_reaction(
    client: &Client,
    config: &RuntimeConfig,
    message_id: Uuid,
    emoji: String,
    active: bool,
) -> Result<(), TransportFailure> {
    let body = ReactionMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        emoji,
    };
    ensure_success(
        authenticated(
            client,
            config,
            if active { Method::POST } else { Method::DELETE },
            &format!("/api/v1/messages/{message_id}/reactions"),
        )
        .json(&body)
        .send()
        .await
        .map_err(request_error)?,
    )
    .await
}

async fn retry_message(
    client: &Client,
    config: &RuntimeConfig,
    chat_id: Option<Uuid>,
    message_id: Uuid,
) -> Result<(), TransportFailure> {
    let chat_id = chat_id.ok_or_else(|| TransportFailure::Protocol("No active chat".to_owned()))?;
    post_empty_mutation(
        client,
        config,
        &format!("/api/v1/chats/{chat_id}/messages/{message_id}/retry"),
    )
    .await
}

async fn execute_context_command(
    client: &Client,
    config: &RuntimeConfig,
    chat_id: Option<Uuid>,
    command: TimelineCommand,
    events: &Sender<DesktopEvent>,
) -> Result<(), TransportFailure> {
    let chat_id = chat_id.ok_or_else(|| TransportFailure::Protocol("No active chat".to_owned()))?;
    let context = match command {
        TimelineCommand::SetInteractionMode(mode) => {
            let body = super::SetInteractionModeRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                mode,
            };
            response_json(
                authenticated(
                    client,
                    config,
                    Method::PUT,
                    &format!("/api/v1/chats/{chat_id}/interaction-mode"),
                )
                .json(&body)
                .send()
                .await
                .map_err(request_error)?,
            )
            .await?
        }
        TimelineCommand::CompactContext {
            strategy,
            target_tokens,
        } => {
            let body = super::CompactWorkingContextRequest {
                request_id: Uuid::now_v7(),
                idempotency_key: Uuid::now_v7(),
                strategy,
                target_tokens,
            };
            post_json(
                client,
                config,
                &format!("/api/v1/chats/{chat_id}/working-context"),
                &body,
            )
            .await?
        }
        _ => unreachable!("only working-context commands are dispatched here"),
    };
    let _ = events.send(DesktopEvent::WorkingContext(context));
    Ok(())
}

async fn create_direct_chat(
    client: &Client,
    config: &RuntimeConfig,
    bot_id: Uuid,
) -> Result<Uuid, TransportFailure> {
    let body = CreateDirectChatRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        bot_id,
    };
    let response: CreateDirectChatResponse = response_json(
        authenticated(client, config, Method::POST, "/api/v1/chats/direct")
            .json(&body)
            .send()
            .await
            .map_err(request_error)?,
    )
    .await?;
    Ok(response.chat.id)
}

async fn post_message(
    client: &Client,
    config: &RuntimeConfig,
    chat_id: Uuid,
    draft: ComposerDraft,
    steer: bool,
) -> Result<(), TransportFailure> {
    let body = SendMessageRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        content: draft.content,
        attachment_ids: draft.attachment_ids,
        reply_to_message_id: draft.reply_to_message_id,
        mentioned_bot_ids: draft.mentioned_bot_ids,
        skill_ids: draft.skill_ids,
        references: draft.references,
    };
    let action = if steer { "steer" } else { "messages" };
    ensure_success(
        authenticated(
            client,
            config,
            Method::POST,
            &format!("/api/v1/chats/{chat_id}/{action}"),
        )
        .json(&body)
        .send()
        .await
        .map_err(request_error)?,
    )
    .await
}

async fn post_empty_mutation(
    client: &Client,
    config: &RuntimeConfig,
    path: &str,
) -> Result<(), TransportFailure> {
    let body = MessageMutationRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
    };
    ensure_success(
        authenticated(client, config, Method::POST, path)
            .json(&body)
            .send()
            .await
            .map_err(request_error)?,
    )
    .await
}

async fn upload_attachment(
    client: &Client,
    config: &RuntimeConfig,
    filename: String,
    media_type: String,
    bytes: Vec<u8>,
) -> Result<Uuid, TransportFailure> {
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let create = CreateAttachmentRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        filename,
        media_type,
        size_bytes: u64::try_from(bytes.len())
            .map_err(|_| TransportFailure::Request("Attachment is too large".to_owned()))?,
        sha256: sha256.clone(),
    };
    let created: CreateAttachmentResponse = response_json(
        authenticated(client, config, Method::POST, "/api/v1/attachments")
            .json(&create)
            .send()
            .await
            .map_err(request_error)?,
    )
    .await?;
    let upload_path = if created.upload_url.starts_with('/') {
        created.upload_url
    } else {
        format!("/{}", created.upload_url)
    };
    ensure_success(
        authenticated(client, config, Method::PUT, &upload_path)
            .body(bytes)
            .send()
            .await
            .map_err(request_error)?,
    )
    .await?;
    let finalize = FinalizeAttachmentRequest {
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::now_v7(),
        sha256,
    };
    ensure_success(
        authenticated(
            client,
            config,
            Method::POST,
            &format!("/api/v1/attachments/{}/finalize", created.attachment_id),
        )
        .json(&finalize)
        .send()
        .await
        .map_err(request_error)?,
    )
    .await?;
    Ok(created.attachment_id)
}

pub(super) fn authenticated(
    client: &Client,
    config: &RuntimeConfig,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    client
        .request(method, format!("{}{}", config.endpoint, path))
        .bearer_auth(&config.device_token)
}

async fn response_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, TransportFailure> {
    let response = checked(response).await?;
    response
        .json()
        .await
        .map_err(|error| TransportFailure::Protocol(error.to_string()))
}

async fn ensure_success(response: reqwest::Response) -> Result<(), TransportFailure> {
    checked(response).await.map(|_| ())
}

async fn checked(response: reqwest::Response) -> Result<reqwest::Response, TransportFailure> {
    match response.status() {
        StatusCode::UNAUTHORIZED => Err(TransportFailure::AuthenticationFailed),
        StatusCode::UPGRADE_REQUIRED => Err(TransportFailure::VersionMismatch),
        status if status.is_success() => Ok(response),
        status => {
            let detail = response
                .json::<ErrorEnvelope>()
                .await
                .map_or_else(|_| status.to_string(), |error| error.message);
            Err(TransportFailure::Request(detail))
        }
    }
}
