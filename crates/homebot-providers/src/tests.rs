//! Acceptance tests for provider-neutral runtime and process supervision.

use super::*;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc, watch};
use uuid::Uuid;

#[derive(Debug)]
struct FakeAdapter {
    id: ProviderAdapterId,
    operations: Arc<Mutex<HashMap<Uuid, watch::Sender<bool>>>>,
    compacted: Arc<Mutex<Vec<String>>>,
    recovered: Vec<Uuid>,
}

impl FakeAdapter {
    fn new() -> Result<Self, ProviderContractError> {
        Ok(Self {
            id: ProviderAdapterId::new("fake")?,
            operations: Arc::new(Mutex::new(HashMap::new())),
            compacted: Arc::new(Mutex::new(Vec::new())),
            recovered: vec![Uuid::nil()],
        })
    }

    async fn run(&self, operation_id: Uuid, conversation_id: String) -> ProviderRun {
        let (events_tx, events_rx) = mpsc::channel(8);
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        self.operations.lock().await.insert(operation_id, cancel_tx);
        let operations = Arc::clone(&self.operations);
        tokio::spawn(async move {
            let _ = events_tx
                .send(ProviderEvent::ConversationStarted { conversation_id })
                .await;
            let _ = events_tx
                .send(ProviderEvent::ContentDelta {
                    text: "hello".to_owned(),
                })
                .await;
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(100)) => {
                    let _ = events_tx.send(ProviderEvent::Usage {
                        usage: ProviderUsage {
                            input_tokens: 2,
                            output_tokens: 1,
                            cached_input_tokens: 0,
                        },
                    }).await;
                    let _ = events_tx.send(ProviderEvent::Completed).await;
                }
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && *cancel_rx.borrow() {
                        let _ = events_tx.send(ProviderEvent::Cancelled).await;
                    }
                }
            }
            operations.lock().await.remove(&operation_id);
        });
        ProviderRun {
            operation_id,
            events: events_rx,
        }
    }
}

#[async_trait::async_trait]
impl ProviderAdapter for FakeAdapter {
    fn id(&self) -> &ProviderAdapterId {
        &self.id
    }

    async fn discover(&self) -> Result<ProviderDescriptor, ProviderError> {
        Ok(ProviderDescriptor {
            adapter_id: self.id.clone(),
            display_name: "Fake Provider".to_owned(),
            executable: None,
            capabilities: ProviderCapabilities {
                supported: [
                    ProviderCapability::ConversationResume,
                    ProviderCapability::Streaming,
                    ProviderCapability::Activities,
                    ProviderCapability::Approvals,
                    ProviderCapability::Cancellation,
                    ProviderCapability::Usage,
                    ProviderCapability::Compaction,
                    ProviderCapability::PlanMode,
                    ProviderCapability::Attachments,
                ]
                .into_iter()
                .collect(),
            },
        })
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth {
            availability: ProviderAvailability::Available,
            message: "Ready".to_owned(),
            checked_at_unix_ms: 1,
        }
    }

    async fn models(&self) -> Result<Vec<ProviderModel>, ProviderError> {
        Ok(vec![ProviderModel {
            id: "test-model".to_owned(),
            display_name: "Test Model".to_owned(),
            context_window_tokens: Some(8_192),
            supports_reasoning: true,
        }])
    }

    async fn start(&self, request: StartRequest) -> Result<ProviderRun, ProviderError> {
        Ok(self
            .run(
                request.operation_id,
                format!("conversation-{}", request.chat_id),
            )
            .await)
    }

    async fn resume(&self, request: ResumeRequest) -> Result<ProviderRun, ProviderError> {
        Ok(self
            .run(request.operation_id, request.conversation_id)
            .await)
    }

    async fn cancel(&self, operation_id: Uuid) -> Result<(), ProviderError> {
        let operation = self
            .operations
            .lock()
            .await
            .get(&operation_id)
            .cloned()
            .ok_or_else(|| ProviderError::internal("Operation is no longer active"))?;
        operation
            .send(true)
            .map_err(|_| ProviderError::internal("Operation is no longer active"))
    }

    async fn resolve_approval(
        &self,
        _approval_id: Uuid,
        _decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        Ok(())
    }

    async fn compact(&self, request: CompactRequest) -> Result<(), ProviderError> {
        self.compacted.lock().await.push(request.conversation_id);
        Ok(())
    }

    async fn recover(&self) -> Result<Vec<ProviderRun>, ProviderError> {
        let mut runs = Vec::new();
        for operation_id in &self.recovered {
            runs.push(
                self.run(*operation_id, "recovered-conversation".to_owned())
                    .await,
            );
        }
        Ok(runs)
    }
}

fn start_request(operation_id: Uuid) -> StartRequest {
    StartRequest {
        operation_id,
        bot_id: Uuid::now_v7(),
        chat_id: Uuid::now_v7(),
        prompt: "Hello".to_owned(),
        model: Some("test-model".to_owned()),
        mode: ExecutionMode::Normal,
        attachments: Vec::new(),
    }
}

#[tokio::test]
async fn runtime_normalizes_discovery_streaming_cancellation_compaction_and_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = ProviderRuntime::new();
    let adapter = Arc::new(FakeAdapter::new()?);
    runtime.register(adapter.clone()).await?;
    assert!(runtime.register(adapter).await.is_err());
    let adapter_id = ProviderAdapterId::new("fake")?;
    assert_eq!(runtime.discover().await?[0].adapter_id, adapter_id);
    assert_eq!(
        runtime.health().await[0].1.availability,
        ProviderAvailability::Available
    );
    assert_eq!(runtime.models(&adapter_id).await?[0].id, "test-model");

    let operation_id = Uuid::now_v7();
    let mut run = runtime
        .start(&adapter_id, start_request(operation_id))
        .await?;
    assert!(matches!(
        run.events.recv().await,
        Some(ProviderEvent::ConversationStarted { .. })
    ));
    assert_eq!(
        run.events.recv().await,
        Some(ProviderEvent::ContentDelta {
            text: "hello".to_owned()
        })
    );
    runtime.cancel(operation_id).await?;
    assert_eq!(run.events.recv().await, Some(ProviderEvent::Cancelled));
    assert!(run.events.recv().await.is_none());
    runtime.finish(operation_id).await;
    assert!(runtime.cancel(operation_id).await.is_err());

    runtime
        .compact(
            &adapter_id,
            CompactRequest {
                conversation_id: "conversation-1".to_owned(),
                target_tokens: Some(1_000),
            },
        )
        .await?;
    let recovered = runtime.recover().await?;
    assert_eq!(
        recovered
            .iter()
            .map(|run| run.operation_id)
            .collect::<Vec<_>>(),
        vec![Uuid::nil()]
    );
    runtime.cancel(Uuid::nil()).await?;
    runtime.finish(Uuid::nil()).await;
    Ok(())
}

#[tokio::test]
async fn runtime_rejects_duplicate_active_ids_and_allows_reuse_after_finish()
-> Result<(), Box<dyn std::error::Error>> {
    let runtime = ProviderRuntime::new();
    let adapter = Arc::new(FakeAdapter::new()?);
    runtime.register(adapter).await?;
    let adapter_id = ProviderAdapterId::new("fake")?;
    let operation_id = Uuid::now_v7();
    let mut run = runtime
        .resume(
            &adapter_id,
            ResumeRequest {
                operation_id,
                conversation_id: "existing".to_owned(),
                prompt: "Continue".to_owned(),
                model: None,
                mode: ExecutionMode::Plan,
                attachments: Vec::new(),
            },
        )
        .await?;
    assert!(
        runtime
            .start(&adapter_id, start_request(operation_id))
            .await
            .is_err()
    );
    runtime.cancel(operation_id).await?;
    while run.events.recv().await.is_some() {}
    runtime.finish(operation_id).await;
    assert!(
        runtime
            .start(&adapter_id, start_request(operation_id))
            .await
            .is_ok()
    );
    Ok(())
}

#[tokio::test]
async fn supervisor_bounds_and_redacts_crash_diagnostics() -> Result<(), Box<dyn std::error::Error>>
{
    let secret = "supersecret";
    let process = SupervisedProcess::spawn(
        ProcessSpec::new("/bin/sh")
            .arg("-c")
            .arg("printf 'first-line\\ntoken=supersecret\\ntail\\n' >&2; exit 7")
            .redact_value(secret)
            .limits(ProcessLimits {
                max_stderr_bytes: 24,
                shutdown_grace: Duration::from_millis(50),
            }),
    )?;
    let report = process.wait().await?;
    assert_eq!(report.termination, ProcessTermination::Crashed);
    assert_eq!(report.exit_code, Some(7));
    assert!(report.stderr_truncated);
    let diagnostics = report.stderr_tail.join("\n");
    assert!(!diagnostics.contains(secret));
    assert!(diagnostics.contains("[REDACTED]"));
    assert!(diagnostics.contains("tail"));
    Ok(())
}

#[tokio::test]
async fn supervisor_stops_reading_an_unterminated_oversized_stderr_frame()
-> Result<(), Box<dyn std::error::Error>> {
    let process = SupervisedProcess::spawn(
        ProcessSpec::new("/bin/sh")
            .arg("-c")
            .arg("head -c 8192 /dev/zero | tr '\\000' x >&2; exit 7")
            .limits(ProcessLimits {
                max_stderr_bytes: 64 * 1024,
                shutdown_grace: Duration::from_millis(50),
            }),
    )?;
    let report = process.wait().await?;
    assert_eq!(report.termination, ProcessTermination::Crashed);
    assert!(report.stderr_truncated);
    assert!(report.stderr_tail.is_empty());
    Ok(())
}

#[tokio::test]
async fn supervisor_prefers_clean_stdin_shutdown_then_enforces_deadline()
-> Result<(), Box<dyn std::error::Error>> {
    let clean = SupervisedProcess::spawn(
        ProcessSpec::new("/bin/sh")
            .arg("-c")
            .arg("while read line; do :; done")
            .limits(ProcessLimits {
                max_stderr_bytes: 1_024,
                shutdown_grace: Duration::from_millis(100),
            }),
    )?;
    let clean_report = clean.shutdown().await?;
    assert_eq!(clean_report.termination, ProcessTermination::CleanShutdown);
    assert_eq!(clean_report.exit_code, Some(0));

    let stubborn = SupervisedProcess::spawn(
        ProcessSpec::new("/bin/sh")
            .arg("-c")
            .arg("while :; do :; done")
            .limits(ProcessLimits {
                max_stderr_bytes: 1_024,
                shutdown_grace: Duration::from_millis(20),
            }),
    )?;
    let killed = stubborn.shutdown().await?;
    assert_eq!(killed.termination, ProcessTermination::KilledAfterGrace);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn supervisor_retries_transient_executable_file_busy()
-> Result<(), Box<dyn std::error::Error>> {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::PermissionsExt, thread};

    let directory = tempfile::tempdir()?;
    let executable = directory.path().join("transiently-busy-provider");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&executable)?;
    file.write_all(b"#!/bin/sh\nexit 0\n")?;
    file.sync_all()?;
    drop(file);
    let mut permissions = std::fs::metadata(&executable)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&executable, permissions)?;

    let busy_handle = OpenOptions::new().write(true).open(&executable)?;
    let release = thread::spawn(move || {
        thread::sleep(Duration::from_millis(5));
        drop(busy_handle);
    });
    let process = SupervisedProcess::spawn(ProcessSpec::new(&executable))?;
    release
        .join()
        .map_err(|_| std::io::Error::other("busy-file release thread panicked"))?;
    assert!(process.wait().await?.succeeded());
    Ok(())
}

#[test]
fn process_debug_output_never_contains_environment_or_redaction_values()
-> Result<(), ProviderContractError> {
    let spec = ProcessSpec::new("provider")
        .arg("--token=secret")
        .environment("API_TOKEN", "secret")
        .redact_value("secret");
    let debug = format!("{spec:?}");
    assert!(!debug.contains("--token=secret"));
    assert!(!debug.contains("API_TOKEN\": \"secret"));
    assert!(!debug.contains("supersecret"));
    ProviderAdapterId::new("valid-adapter")?;
    assert!(ProviderAdapterId::new("Invalid Adapter").is_err());
    Ok(())
}
