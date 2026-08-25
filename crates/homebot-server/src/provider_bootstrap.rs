//! Production provider registry bootstrap.
//!
//! Provider-native configuration stays on the server. Clients only receive the
//! safe profile projection included in the authoritative snapshot.

use homebot_providers::{
    ClaudeAdapter, ClaudeProfile, CodexAdapter, CodexProfile, GenericProcessAdapter,
    GenericProcessProfile, OpenAiApiStyle, OpenAiCompatibleAdapter, OpenAiCompatibleProfile,
    ProviderAdapter, ProviderAdapterId, ProviderRuntime, SecretReference,
};
use homebot_secrets::{OsSecretVault, VaultProviderSecretResolver};
use homebot_storage::{ProviderProfileRecord, Storage};
use serde::Deserialize;
use serde_json::json;
use std::{path::Path, sync::Arc};
use uuid::Uuid;

use crate::AppState;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const CODEX_PROFILE_ID: Uuid = Uuid::from_u128(0x5b9b_e1a1_357a_4b77_9159_4d89_5918_4a01);
const CLAUDE_PROFILE_ID: Uuid = Uuid::from_u128(0x1514_3b31_fbf9_49b6_99ba_5e76_635f_5d02);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBootstrapConfig {
    #[serde(default = "default_profiles")]
    pub profiles: Vec<LocalCliProfile>,
}

impl Default for ProviderBootstrapConfig {
    fn default() -> Self {
        Self {
            profiles: default_profiles(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCliProfile {
    pub id: Uuid,
    pub adapter_id: String,
    pub kind: LocalCliKind,
    pub display_name: String,
    #[serde(default)]
    pub binary: Option<String>,
    #[serde(default)]
    pub working_directory: Option<std::path::PathBuf>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_style: Option<ConfiguredOpenAiApiStyle>,
    #[serde(default)]
    pub secret_reference_id: Option<Uuid>,
    #[serde(default)]
    pub arguments: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalCliKind {
    Codex,
    ClaudeCode,
    OpenAiCompatible,
    GenericProcess,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfiguredOpenAiApiStyle {
    Responses,
    ChatCompletions,
}

/// Loads a bounded, secret-free JSON config. Without a file, `HomeBot` exposes
/// deterministic Codex and Claude Code profiles whose health reports whether
/// their executables and authentication are available.
///
/// # Errors
/// Returns an I/O or strict JSON validation error for an invalid file.
pub fn load_config(path: Option<&Path>) -> anyhow::Result<ProviderBootstrapConfig> {
    let Some(path) = path else {
        return Ok(ProviderBootstrapConfig::default());
    };
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        anyhow::bail!("HomeBot provider config must be a regular file no larger than 1 MiB");
    }
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(Into::into)
}

/// Persists safe profile metadata and registers every configured adapter.
///
/// # Errors
/// Returns a configuration, adapter registration, or persistence error.
pub async fn build_runtime(
    storage: &Storage,
    config: ProviderBootstrapConfig,
) -> anyhow::Result<Arc<ProviderRuntime>> {
    let runtime = Arc::new(ProviderRuntime::new());
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX);
    for profile in config.profiles {
        let adapter_id = ProviderAdapterId::new(profile.adapter_id.clone())?;
        if profile.display_name.trim().is_empty() {
            anyhow::bail!("provider display name must not be empty");
        }
        let provider_kind = match profile.kind {
            LocalCliKind::Codex => "codex",
            LocalCliKind::ClaudeCode => "claude-code",
            LocalCliKind::OpenAiCompatible => "openai-compatible",
            LocalCliKind::GenericProcess => "generic-process",
        };
        let configuration = json!({
            "model": profile.model.clone(),
            "provider_kind": provider_kind,
            "base_url": profile.base_url.clone(),
            "api_style": profile.api_style.map(|style| match style {
                ConfiguredOpenAiApiStyle::Responses => "responses",
                ConfiguredOpenAiApiStyle::ChatCompletions => "chat-completions",
            }),
        });
        let record = ProviderProfileRecord {
            id: profile.id,
            adapter_kind: profile.adapter_id.clone(),
            display_name: profile.display_name.clone(),
            configuration,
            secret_reference_id: profile.secret_reference_id,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        let adapter = configured_adapter(&profile, adapter_id)?;
        runtime.register(adapter).await?;
        storage.upsert_provider_profile(&record).await?;
    }
    Ok(runtime)
}

fn configured_adapter(
    profile: &LocalCliProfile,
    adapter_id: ProviderAdapterId,
) -> anyhow::Result<Arc<dyn ProviderAdapter>> {
    match profile.kind {
        LocalCliKind::Codex => {
            let binary = required(profile.binary.as_ref(), "Codex profile binary")?;
            let mut configured = CodexProfile::new(adapter_id, binary);
            if let Some(directory) = &profile.working_directory {
                configured = configured.working_directory(directory);
            }
            Ok(Arc::new(CodexAdapter::new(configured)))
        }
        LocalCliKind::ClaudeCode => {
            let binary = required(profile.binary.as_ref(), "Claude Code profile binary")?;
            let mut configured = ClaudeProfile::new(adapter_id, binary);
            if let Some(directory) = &profile.working_directory {
                configured = configured.working_directory(directory);
            }
            Ok(Arc::new(ClaudeAdapter::new(configured)))
        }
        LocalCliKind::OpenAiCompatible => {
            let base_url = required(profile.base_url.as_ref(), "OpenAI-compatible base URL")?
                .parse::<url::Url>()?;
            let secret_reference_id = profile.secret_reference_id.ok_or_else(|| {
                anyhow::anyhow!("OpenAI-compatible profile requires secret_reference_id")
            })?;
            let model = profile
                .model
                .clone()
                .ok_or_else(|| anyhow::anyhow!("OpenAI-compatible profile requires a model"))?;
            let api_style = match profile
                .api_style
                .ok_or_else(|| anyhow::anyhow!("OpenAI-compatible profile requires api_style"))?
            {
                ConfiguredOpenAiApiStyle::Responses => OpenAiApiStyle::Responses,
                ConfiguredOpenAiApiStyle::ChatCompletions => OpenAiApiStyle::ChatCompletions,
            };
            let configured = OpenAiCompatibleProfile::new(
                adapter_id,
                profile.display_name.clone(),
                base_url,
                api_style,
                SecretReference::new(secret_reference_id),
                model,
            )?;
            let resolver = Arc::new(VaultProviderSecretResolver::new(Arc::new(
                OsSecretVault::new(),
            )));
            Ok(Arc::new(OpenAiCompatibleAdapter::new(
                configured, resolver,
            )?))
        }
        LocalCliKind::GenericProcess => {
            let binary = required(profile.binary.as_ref(), "generic process binary")?;
            let mut configured =
                GenericProcessProfile::new(adapter_id, profile.display_name.clone(), binary);
            for argument in &profile.arguments {
                configured = configured.argument(argument);
            }
            if let Some(directory) = &profile.working_directory {
                configured = configured.working_directory(directory);
            }
            Ok(Arc::new(GenericProcessAdapter::new(configured)))
        }
    }
}

fn required<'a>(value: Option<&'a String>, field: &str) -> anyhow::Result<&'a str> {
    value
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{field} must not be empty"))
}

/// Production composition root shared by the binary and integration tests.
///
/// # Errors
/// Returns any provider configuration, registration, or persistence error.
pub async fn compose_app_state(
    storage: Storage,
    owner_token: &str,
    artifact_root: std::path::PathBuf,
    config: ProviderBootstrapConfig,
) -> anyhow::Result<AppState> {
    storage
        .recover_interrupted_chat_turns(Uuid::nil(), crate::unix_time_ms())
        .await?;
    let runtime = build_runtime(&storage, config).await?;
    Ok(AppState::new(storage, owner_token)
        .with_artifact_root(artifact_root)
        .with_provider_runtime(runtime))
}

fn default_profiles() -> Vec<LocalCliProfile> {
    vec![
        LocalCliProfile {
            id: CODEX_PROFILE_ID,
            adapter_id: "codex".to_owned(),
            kind: LocalCliKind::Codex,
            display_name: "Codex CLI".to_owned(),
            binary: Some("codex".to_owned()),
            working_directory: None,
            model: None,
            base_url: None,
            api_style: None,
            secret_reference_id: None,
            arguments: Vec::new(),
        },
        LocalCliProfile {
            id: CLAUDE_PROFILE_ID,
            adapter_id: "claude-code".to_owned(),
            kind: LocalCliKind::ClaudeCode,
            display_name: "Claude Code".to_owned(),
            binary: Some("claude".to_owned()),
            working_directory: None,
            model: None,
            base_url: None,
            api_style: None,
            secret_reference_id: None,
            arguments: Vec::new(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use homebot_providers::{ExecutionMode, ProviderEvent, StartRequest};

    #[tokio::test]
    async fn defaults_are_persisted_and_registered_even_when_clis_are_absent() -> anyhow::Result<()>
    {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let runtime = build_runtime(&storage, ProviderBootstrapConfig::default()).await?;
        let profiles = storage.provider_profiles().await?;
        assert_eq!(profiles.len(), 2);
        assert_eq!(runtime.health().await.len(), 2);
        assert!(
            profiles
                .iter()
                .all(|profile| profile.secret_reference_id.is_none())
        );
        Ok(())
    }

    #[tokio::test]
    async fn production_startup_fails_orphaned_direct_and_group_provider_turns()
    -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let bot = storage
            .create_bot(
                Uuid::nil(),
                homebot_domain::Bot::create("Scout", "Research")?,
                1,
            )
            .await?;
        let chat = storage
            .create_direct_chat(Uuid::nil(), bot.id.0, Uuid::now_v7(), 2)
            .await?;
        storage
            .set_chat_running(Uuid::nil(), chat.id, true, 3)
            .await?;
        let message_id = Uuid::now_v7();
        storage
            .create_bot_message(Uuid::nil(), chat.id, bot.id.0, message_id, 4)
            .await?;
        let reviewer = storage
            .create_bot(
                Uuid::nil(),
                homebot_domain::Bot::create("Reviewer", "Review")?,
                5,
            )
            .await?;
        let group_id = Uuid::now_v7();
        storage
            .create_group_chat(
                Uuid::nil(),
                group_id,
                "Team",
                &[bot.id.0, reviewer.id.0],
                bot.id.0,
                4,
                2,
                6,
            )
            .await?;
        let group_operation_id = Uuid::now_v7();
        storage
            .set_group_bot_status(
                Uuid::nil(),
                group_id,
                reviewer.id.0,
                homebot_domain::chat::GroupBotStatus::Running,
                Some(group_operation_id),
                7,
            )
            .await?;
        let group_message_id = Uuid::now_v7();
        storage
            .create_bot_message(Uuid::nil(), group_id, reviewer.id.0, group_message_id, 8)
            .await?;

        let _state = compose_app_state(
            storage.clone(),
            "owner-token",
            directory.path().join("artifacts"),
            ProviderBootstrapConfig {
                profiles: Vec::new(),
            },
        )
        .await?;

        let message = storage.message(Uuid::nil(), message_id).await?;
        assert_eq!(message.status, homebot_domain::chat::MessageStatus::Failed);
        assert!(
            message
                .error_json
                .as_ref()
                .is_some_and(|error| error["retryable"] == true)
        );
        assert!(!storage.get_direct_chat(Uuid::nil(), chat.id).await?.running);
        assert_eq!(
            storage.message(Uuid::nil(), group_message_id).await?.status,
            homebot_domain::chat::MessageStatus::Failed
        );
        let reviewer_state = storage
            .group_participants(Uuid::nil(), group_id)
            .await?
            .into_iter()
            .find(|participant| participant.bot_id == reviewer.id.0)
            .ok_or_else(|| anyhow::anyhow!("Reviewer group state missing"))?;
        assert_eq!(
            reviewer_state.status,
            homebot_domain::chat::GroupBotStatus::Failed
        );
        assert!(reviewer_state.active_operation_id.is_none());
        Ok(())
    }

    #[test]
    fn rejects_unknown_or_oversized_configuration() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let config = directory.path().join("providers.json");
        std::fs::write(&config, br#"{"unknown":true}"#)?;
        assert!(load_config(Some(&config)).is_err());
        let oversized = usize::try_from(MAX_CONFIG_BYTES + 1)?;
        std::fs::write(&config, vec![b'x'; oversized])?;
        assert!(load_config(Some(&config)).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn invalid_profile_fails_without_becoming_a_production_default() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let invalid = ProviderBootstrapConfig {
            profiles: vec![LocalCliProfile {
                id: Uuid::now_v7(),
                adapter_id: "fixture".to_owned(),
                kind: LocalCliKind::GenericProcess,
                display_name: "Fixture".to_owned(),
                binary: None,
                working_directory: None,
                model: None,
                base_url: None,
                api_style: None,
                secret_reference_id: None,
                arguments: Vec::new(),
            }],
        };
        assert!(build_runtime(&storage, invalid).await.is_err());
        assert!(storage.provider_profiles().await?.is_empty());
        let defaults = ProviderBootstrapConfig::default();
        assert!(defaults.profiles.iter().all(|profile| {
            matches!(profile.kind, LocalCliKind::Codex | LocalCliKind::ClaudeCode)
                && !profile.adapter_id.contains("fixture")
        }));
        Ok(())
    }

    #[tokio::test]
    async fn openai_profile_registers_only_an_opaque_secret_reference() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let storage = Storage::open(&directory.path().join("homebot.db")).await?;
        let reference = Uuid::now_v7();
        let adapter_id = "private-openai";
        let runtime = build_runtime(
            &storage,
            ProviderBootstrapConfig {
                profiles: vec![LocalCliProfile {
                    id: Uuid::now_v7(),
                    adapter_id: adapter_id.to_owned(),
                    kind: LocalCliKind::OpenAiCompatible,
                    display_name: "Private API".to_owned(),
                    binary: None,
                    working_directory: None,
                    model: Some("model-1".to_owned()),
                    base_url: Some("https://models.example.test/v1".to_owned()),
                    api_style: Some(ConfiguredOpenAiApiStyle::Responses),
                    secret_reference_id: Some(reference),
                    arguments: Vec::new(),
                }],
            },
        )
        .await?;
        let descriptor = runtime
            .descriptor(&ProviderAdapterId::new(adapter_id)?)
            .await?;
        assert_eq!(descriptor.adapter_id.as_str(), adapter_id);
        let persisted = storage.provider_profiles().await?;
        assert_eq!(persisted[0].secret_reference_id, Some(reference));
        assert!(!persisted[0].configuration.to_string().contains("secret"));
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn configured_cli_executes_through_production_runtime_and_survives_restart()
    -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir()?;
        let database = directory.path().join("homebot.db");
        let binary = directory.path().join("claude-fixture");
        std::fs::write(
            &binary,
            r#"#!/bin/sh
IFS= read -r input
printf '%s\n' '{"type":"system","subtype":"init","session_id":"production_fixture"}'
printf '%s\n' '{"type":"stream_event","session_id":"production_fixture","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"ready"}}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"session_id":"production_fixture","usage":{"input_tokens":1,"output_tokens":1}}'
"#,
        )?;
        let mut permissions = std::fs::metadata(&binary)?.permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&binary, permissions)?;
        let profile_id = Uuid::now_v7();
        let configured = ProviderBootstrapConfig {
            profiles: vec![LocalCliProfile {
                id: profile_id,
                adapter_id: "claude-production-fixture".to_owned(),
                kind: LocalCliKind::ClaudeCode,
                display_name: "Claude fixture".to_owned(),
                binary: Some(binary.to_string_lossy().into_owned()),
                working_directory: None,
                model: None,
                base_url: None,
                api_style: None,
                secret_reference_id: None,
                arguments: Vec::new(),
            }],
        };
        let storage = Storage::open(&database).await?;
        let state = compose_app_state(
            storage.clone(),
            "owner-token",
            directory.path().join("artifacts"),
            configured.clone(),
        )
        .await?;
        let adapter_id = ProviderAdapterId::new("claude-production-fixture")?;
        let mut bot = homebot_domain::Bot::create("Production Bot", "Provider wiring")?;
        bot.provider_profile_id = Some(profile_id);
        let bot = storage.create_bot(Uuid::nil(), bot, 1).await?;
        let route = storage
            .provider_route_for_bot(Uuid::nil(), bot.id.0)
            .await?
            .ok_or_else(|| anyhow::anyhow!("configured Bot is missing its provider route"))?;
        assert_eq!(route.adapter_kind, adapter_id.as_str());
        let mut run = state
            .provider_runtime()
            .start(
                &adapter_id,
                StartRequest {
                    operation_id: Uuid::now_v7(),
                    bot_id: Uuid::now_v7(),
                    chat_id: Uuid::now_v7(),
                    prompt: "Hello".to_owned(),
                    model: None,
                    working_directory: None,
                    mode: ExecutionMode::Normal,
                    attachments: Vec::new(),
                    tools: Vec::new(),
                },
            )
            .await?;
        assert!(
            matches!(run.events.recv().await, Some(ProviderEvent::ConversationStarted { conversation_id }) if conversation_id == "production_fixture")
        );
        assert_eq!(
            run.events.recv().await,
            Some(ProviderEvent::ContentDelta {
                text: "ready".to_owned()
            })
        );
        assert!(matches!(
            run.events.recv().await,
            Some(ProviderEvent::Usage { .. })
        ));
        assert_eq!(run.events.recv().await, Some(ProviderEvent::Completed));
        drop(state);
        drop(storage);

        let reopened = Storage::open(&database).await?;
        let restarted = build_runtime(&reopened, configured).await?;
        assert_eq!(reopened.provider_profiles().await?[0].id, profile_id);
        let mut restarted_run = restarted
            .start(
                &adapter_id,
                StartRequest {
                    operation_id: Uuid::now_v7(),
                    bot_id: Uuid::now_v7(),
                    chat_id: Uuid::now_v7(),
                    prompt: "After restart".to_owned(),
                    model: None,
                    working_directory: None,
                    mode: ExecutionMode::Normal,
                    attachments: Vec::new(),
                    tools: Vec::new(),
                },
            )
            .await?;
        assert!(matches!(
            restarted_run.events.recv().await,
            Some(ProviderEvent::ConversationStarted { .. })
        ));
        Ok(())
    }
}
