use super::{
    ApprovalDecisionRequest, BotClientCommand, BotEditorDraft, BotMutationRequest, BotResponse,
    Client, ComposerDraft, CreateAttachmentRequest, CreateAttachmentResponse, CreateBotRequest,
    CreateDirectChatRequest, CreateDirectChatResponse, DesktopCommand, DesktopEvent, Digest,
    ErrorEnvelope, FinalizeAttachmentRequest, MessageMutationRequest, Method, RuntimeConfig,
    SendMessageRequest, Sender, Sha256, StatusCode, TimelineCommand, TransportFailure,
    UpdateBotRequest, Uuid, WorkspaceCommand, protocol_error, request_error,
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
        DesktopCommand::Shutdown => Ok(()),
    }
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
    }
}

async fn execute_bot(
    client: &Client,
    config: &RuntimeConfig,
    command: BotClientCommand,
    events: &Sender<DesktopEvent>,
) -> Result<(), TransportFailure> {
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
            let chat_id =
                chat_id.ok_or_else(|| TransportFailure::Protocol("No active chat".to_owned()))?;
            post_empty_mutation(
                client,
                config,
                &format!("/api/v1/chats/{chat_id}/messages/{message_id}/retry"),
            )
            .await
        }
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
    }
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
