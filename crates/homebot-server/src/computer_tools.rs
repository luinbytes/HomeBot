//! Provider-neutral, server-owned workspace and terminal tools.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use homebot_providers::{ProviderTool, ProviderToolCall, ProviderToolResult};
use homebot_tools::{
    FilesystemLimits, NoopActivitySink, OperationContext, ScopedFilesystem, TerminalChunk,
    TerminalCommand, TerminalLimits, TerminalService, ToolError,
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use uuid::Uuid;

use crate::AppState;

const LIST: &str = "homebot_list_files";
const READ: &str = "homebot_read_file";
const WRITE: &str = "homebot_write_file";
const MKDIR: &str = "homebot_create_directory";
const RUN: &str = "homebot_run_command";
const MAX_FILE_BYTES: usize = 256 * 1024;

pub(super) enum ComputerProviderOutcome {
    Result(ProviderToolResult),
    Cancelled,
}

pub(super) fn provider_tools() -> Vec<ProviderTool> {
    let path = serde_json::json!({"type":"string","minLength":1,"maxLength":1024,"description":"Path relative to this Bot's HomeBot workspace"});
    vec![
        ProviderTool { name: LIST.to_owned(), description: "List one directory in this Bot's server-owned workspace. Paths are relative; traversal and symlinks are rejected.".to_owned(), input_schema: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"path":{"type":"string","maxLength":1024,"default":""}}}) },
        ProviderTool { name: READ.to_owned(), description: "Read one bounded file from this Bot's server-owned workspace.".to_owned(), input_schema: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"path":path.clone()},"required":["path"]}) },
        ProviderTool { name: WRITE.to_owned(), description: "Atomically write one bounded UTF-8 file in this Bot's workspace. HomeBot may require owner approval.".to_owned(), input_schema: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"path":path.clone(),"content":{"type":"string","maxLength":MAX_FILE_BYTES}},"required":["path","content"]}) },
        ProviderTool { name: MKDIR.to_owned(), description: "Create one directory in this Bot's workspace. HomeBot may require owner approval.".to_owned(), input_schema: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"path":path.clone()},"required":["path"]}) },
        ProviderTool { name: RUN.to_owned(), description: "Run one explicit absolute executable in a bounded server PTY inside this Bot's workspace. No shell or environment is implied; HomeBot may require owner approval.".to_owned(), input_schema: serde_json::json!({"type":"object","additionalProperties":false,"properties":{"program":{"type":"string","minLength":1,"maxLength":512,"description":"Existing absolute executable path"},"arguments":{"type":"array","maxItems":64,"items":{"type":"string","maxLength":4096}},"working_directory":{"type":"string","maxLength":1024,"default":""}},"required":["program"]}) },
    ]
}

pub(super) async fn handle_provider_tool(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    bot_id: Uuid,
    message_id: Uuid,
    call: &ProviderToolCall,
) -> Option<ComputerProviderOutcome> {
    if !matches!(call.name.as_str(), LIST | READ | WRITE | MKDIR | RUN) {
        return None;
    }
    Some(
        match call_tool(state, operation_id, chat_id, bot_id, message_id, call).await {
            Ok(Some(content)) => ComputerProviderOutcome::Result(ProviderToolResult {
                success: true,
                content,
            }),
            Ok(None) => ComputerProviderOutcome::Cancelled,
            Err(content) => ComputerProviderOutcome::Result(ProviderToolResult {
                success: false,
                content,
            }),
        },
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArguments {
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteArguments {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunArguments {
    program: String,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    working_directory: String,
}

#[allow(clippy::too_many_lines)]
async fn call_tool(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    bot_id: Uuid,
    message_id: Uuid,
    call: &ProviderToolCall,
) -> Result<Option<String>, String> {
    let workspace = crate::provider_turn::provider_working_directory(state, chat_id, bot_id)
        .await
        .map_err(|_| "HomeBot could not open the Bot workspace".to_owned())?
        .ok_or_else(|| "The Bot workspace is unavailable".to_owned())?;
    let context = operation_context(state, operation_id, chat_id, bot_id).await?;
    if call.name == RUN {
        let arguments: RunArguments = decode(&call.arguments, "Invalid terminal arguments")?;
        return run_command(state, message_id, workspace, context, arguments).await;
    }
    let filesystem = ScopedFilesystem::new(
        &workspace,
        Arc::clone(&state.policy_engine),
        Arc::new(NoopActivitySink),
        FilesystemLimits {
            max_read_bytes: MAX_FILE_BYTES,
            max_write_bytes: MAX_FILE_BYTES,
            max_directory_entries: 1_000,
        },
    )
    .map_err(|error| tool_error(&error))?;
    match call.name.as_str() {
        LIST => {
            let arguments: PathArguments = decode(&call.arguments, "Invalid directory path")?;
            let path = arguments.path;
            let entries = approved_operation(
                state,
                operation_id,
                chat_id,
                message_id,
                "homebot.filesystem.read",
                "List workspace directory",
                || filesystem.list(context.clone(), &path, None),
                |approval_id| filesystem.list(context.clone(), &path, Some(approval_id)),
            )
            .await?;
            entries
                .map(|entries| {
                    serde_json::to_string(&entries)
                        .map_err(|_| "HomeBot could not encode the directory listing".to_owned())
                })
                .transpose()
        }
        READ => {
            let arguments: PathArguments = decode(&call.arguments, "Invalid file path")?;
            let path = arguments.path;
            let bytes = approved_operation(
                state,
                operation_id,
                chat_id,
                message_id,
                "homebot.filesystem.read",
                "Read workspace file",
                || filesystem.read(context.clone(), &path, None),
                |approval_id| filesystem.read(context.clone(), &path, Some(approval_id)),
            )
            .await?;
            Ok(bytes.map(|bytes| match String::from_utf8(bytes) {
                Ok(text) => serde_json::json!({"encoding":"utf8","content":text}).to_string(),
                Err(error) => serde_json::json!({"encoding":"base64","content":STANDARD.encode(error.into_bytes())}).to_string(),
            }))
        }
        WRITE => {
            let arguments: WriteArguments = decode(&call.arguments, "Invalid file write")?;
            let contents = arguments.content.into_bytes();
            let path = arguments.path;
            let result = approved_operation(
                state,
                operation_id,
                chat_id,
                message_id,
                "homebot.filesystem.write",
                "Write workspace file",
                || filesystem.write(context.clone(), &path, contents.clone(), None),
                |approval_id| {
                    filesystem.write(context.clone(), &path, contents.clone(), Some(approval_id))
                },
            )
            .await?;
            Ok(result.map(|()| serde_json::json!({"written":true,"path":path}).to_string()))
        }
        MKDIR => {
            let arguments: PathArguments = decode(&call.arguments, "Invalid directory path")?;
            let path = arguments.path;
            let result = approved_operation(
                state,
                operation_id,
                chat_id,
                message_id,
                "homebot.filesystem.write",
                "Create workspace directory",
                || filesystem.create_directory(context.clone(), &path, None),
                |approval_id| {
                    filesystem.create_directory(context.clone(), &path, Some(approval_id))
                },
            )
            .await?;
            Ok(result.map(|()| serde_json::json!({"created":true,"path":path}).to_string()))
        }
        _ => Err("Unknown HomeBot computer tool".to_owned()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn approved_operation<T, F, Fut, R, RFut>(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    message_id: Uuid,
    capability: &str,
    title: &str,
    first: F,
    retry: R,
) -> Result<Option<T>, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, ToolError>>,
    R: FnOnce(Uuid) -> RFut,
    RFut: std::future::Future<Output = Result<T, ToolError>>,
{
    match first().await {
        Ok(value) => Ok(Some(value)),
        Err(ToolError::ApprovalRequired(ticket)) => {
            let Some(approval_id) = crate::provider_turn::await_capability_approval(
                state,
                operation_id,
                chat_id,
                message_id,
                &ticket,
                capability,
                title,
            )
            .await?
            else {
                return Ok(None);
            };
            retry(approval_id)
                .await
                .map(Some)
                .map_err(|error| tool_error(&error))
        }
        Err(error) => Err(tool_error(&error)),
    }
}

#[allow(clippy::too_many_lines)]
async fn run_command(
    state: &AppState,
    message_id: Uuid,
    workspace: PathBuf,
    context: OperationContext,
    arguments: RunArguments,
) -> Result<Option<String>, String> {
    if arguments.arguments.len() > 64 || arguments.arguments.iter().any(|value| value.len() > 4_096)
    {
        return Err("Terminal arguments exceed HomeBot limits".to_owned());
    }
    let limits = TerminalLimits {
        max_output_bytes: 256 * 1024,
        max_input_bytes: 1,
        max_runtime: Duration::from_secs(120),
        max_concurrent_processes: 1,
        allowed_environment: BTreeSet::new(),
    };
    let service = Arc::new(
        TerminalService::new(
            &workspace,
            Arc::clone(&state.policy_engine),
            Arc::new(NoopActivitySink),
            limits,
        )
        .map_err(|error| tool_error(&error))?,
    );
    state
        .computer_terminals
        .lock()
        .await
        .insert(context.operation_id, Arc::clone(&service));
    let command = TerminalCommand {
        program: PathBuf::from(arguments.program),
        arguments: arguments.arguments,
        working_directory: PathBuf::from(arguments.working_directory),
        environment: BTreeMap::new(),
        rows: 24,
        columns: 100,
    };
    let mut run = match service.start(context.clone(), command.clone(), None).await {
        Ok(run) => run,
        Err(ToolError::ApprovalRequired(ticket)) => {
            let approval = crate::provider_turn::await_capability_approval(
                state,
                context.operation_id,
                context.chat_id,
                message_id,
                &ticket,
                "homebot.terminal.execute",
                "Run workspace command",
            )
            .await;
            let Some(approval_id) = (match approval {
                Ok(approval) => approval,
                Err(error) => {
                    state
                        .computer_terminals
                        .lock()
                        .await
                        .remove(&context.operation_id);
                    return Err(error);
                }
            }) else {
                state
                    .computer_terminals
                    .lock()
                    .await
                    .remove(&context.operation_id);
                return Ok(None);
            };
            match service
                .start(context.clone(), command, Some(approval_id))
                .await
            {
                Ok(run) => run,
                Err(error) => {
                    state
                        .computer_terminals
                        .lock()
                        .await
                        .remove(&context.operation_id);
                    return Err(tool_error(&error));
                }
            }
        }
        Err(error) => {
            state
                .computer_terminals
                .lock()
                .await
                .remove(&context.operation_id);
            return Err(tool_error(&error));
        }
    };
    let mut output = Vec::new();
    let mut terminal =
        serde_json::json!({"success":false,"reason":"terminal_ended_without_status"});
    while let Some(chunk) = run.events.recv().await {
        match chunk {
            TerminalChunk::Output { bytes } => output.extend(bytes),
            TerminalChunk::Exited { exit_code, success } => {
                terminal = serde_json::json!({"success":success,"exit_code":exit_code});
            }
            TerminalChunk::Cancelled => {
                terminal = serde_json::json!({"success":false,"reason":"cancelled"});
            }
            TerminalChunk::TimedOut => {
                terminal = serde_json::json!({"success":false,"reason":"timed_out"});
            }
            TerminalChunk::Failed { reason } => {
                terminal = serde_json::json!({"success":false,"reason":reason});
            }
            TerminalChunk::Started { .. } => {}
        }
    }
    state
        .computer_terminals
        .lock()
        .await
        .remove(&context.operation_id);
    terminal["output"] = serde_json::Value::String(String::from_utf8_lossy(&output).into_owned());
    Ok(Some(terminal.to_string()))
}

pub(super) async fn cancel(state: &AppState, operation_id: Uuid) {
    if let Some(service) = state
        .computer_terminals
        .lock()
        .await
        .get(&operation_id)
        .cloned()
    {
        let _ = service.cancel(operation_id).await;
    }
}

async fn operation_context(
    state: &AppState,
    operation_id: Uuid,
    chat_id: Uuid,
    bot_id: Uuid,
) -> Result<OperationContext, String> {
    let workspace_id = state
        .storage
        .chat_workspace(state.owner_id, chat_id)
        .await
        .map_err(|_| "HomeBot could not read the chat workspace".to_owned())?
        .map_or(Uuid::nil(), |workspace| workspace.workspace_id);
    Ok(OperationContext {
        operation_id,
        owner_id: state.owner_id,
        device_id: Uuid::nil(),
        bot_id,
        chat_id,
        workspace_id,
    })
}

fn decode<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    message: &str,
) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|_| message.to_owned())
}

fn tool_error(error: &ToolError) -> String {
    match error {
        ToolError::Denied => "The owner denied this computer action".to_owned(),
        ToolError::InvalidApproval => "The computer approval is no longer valid".to_owned(),
        ToolError::PathOutsideWorkspace | ToolError::SymlinkRejected => {
            "The path is outside the safe Bot workspace".to_owned()
        }
        ToolError::LimitExceeded => {
            "The computer action exceeded HomeBot's bounded limits".to_owned()
        }
        ToolError::InvalidRequest(message) => format!("Invalid computer request: {message}"),
        _ => "HomeBot could not complete the computer action safely".to_owned(),
    }
}
