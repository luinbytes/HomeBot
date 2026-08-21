use super::*;
use axum::{
    Json, Router,
    body::{Body, Bytes},
    http::{HeaderMap, Response, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use futures_util::{StreamExt, stream};
use std::{
    convert::Infallible,
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Debug)]
struct TestSecrets {
    value: String,
    resolutions: AtomicUsize,
}

#[async_trait::async_trait]
impl ProviderSecretResolver for TestSecrets {
    async fn resolve(
        &self,
        _reference: SecretReference,
    ) -> Result<crate::ResolvedSecret, ProviderError> {
        self.resolutions.fetch_add(1, Ordering::Relaxed);
        Ok(crate::ResolvedSecret::new(self.value.clone()))
    }
}

#[tokio::test]
async fn responses_profile_resolves_secret_at_request_time_and_streams()
-> Result<(), Box<dyn std::error::Error>> {
    let secret = "fixture-secret";
    let app = Router::new()
        .route(
            "/v1/models",
            get(move |headers: HeaderMap| async move {
                if headers.get("authorization").and_then(|value| value.to_str().ok())
                    == Some("Bearer fixture-secret")
                {
                    Json(json!({"data": [{"id": "model-fixture"}]})).into_response()
                } else {
                    StatusCode::UNAUTHORIZED.into_response()
                }
            }),
        )
        .route(
            "/v1/responses",
            post(move |headers: HeaderMap| async move {
                if headers.get("authorization").and_then(|value| value.to_str().ok())
                    != Some("Bearer fixture-secret")
                {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from(concat!(
                        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_fixture\"}}\n\n",
                        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
                        "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":2,\"output_tokens\":1}}}\n\n"
                    )))
                    .unwrap_or_else(|_| Response::new(Body::empty()))
                    .into_response()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let secrets = Arc::new(TestSecrets {
        value: secret.to_owned(),
        resolutions: AtomicUsize::new(0),
    });
    let profile = OpenAiCompatibleProfile::new(
        ProviderAdapterId::new("openai-fixture")?,
        "Fixture API",
        Url::parse(&format!("http://{address}/v1/"))?,
        OpenAiApiStyle::Responses,
        SecretReference::new(Uuid::now_v7()),
        "model-fixture",
    )?;
    let adapter = OpenAiCompatibleAdapter::new(profile, secrets.clone())?;
    assert_eq!(adapter.models().await?[0].id, "model-fixture");
    let mut run = adapter
        .start(StartRequest {
            operation_id: Uuid::now_v7(),
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            prompt: "Hello".to_owned(),
            model: None,
            mode: ExecutionMode::Normal,
            attachments: Vec::new(),
        })
        .await?;
    assert!(
        matches!(run.events.recv().await, Some(ProviderEvent::ConversationStarted { conversation_id }) if conversation_id == "resp_fixture")
    );
    assert_eq!(
        run.events.recv().await,
        Some(ProviderEvent::ContentDelta {
            text: "Hello".to_owned()
        })
    );
    assert!(matches!(
        run.events.recv().await,
        Some(ProviderEvent::Usage { .. })
    ));
    assert_eq!(run.events.recv().await, Some(ProviderEvent::Completed));
    assert_eq!(secrets.resolutions.load(Ordering::Relaxed), 2);
    assert!(!format!("{adapter:?}").contains(secret));
    Ok(())
}

#[tokio::test]
async fn cancellation_stops_an_openai_compatible_stream() -> Result<(), Box<dyn std::error::Error>>
{
    let app = Router::new().route(
        "/v1/responses",
        post(|| async move {
            let first = stream::once(async {
                Ok::<_, Infallible>(Bytes::from_static(
                    b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_pending\"}}\n\n",
                ))
            });
            let pending = stream::pending::<Result<Bytes, Infallible>>();
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(first.chain(pending)))
                .unwrap_or_else(|_| Response::new(Body::empty()))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let secrets = Arc::new(TestSecrets {
        value: "fixture-secret".to_owned(),
        resolutions: AtomicUsize::new(0),
    });
    let adapter = OpenAiCompatibleAdapter::new(
        OpenAiCompatibleProfile::new(
            ProviderAdapterId::new("openai-cancel")?,
            "Fixture API",
            Url::parse(&format!("http://{address}/v1/"))?,
            OpenAiApiStyle::Responses,
            SecretReference::new(Uuid::now_v7()),
            "model-fixture",
        )?,
        secrets,
    )?;
    let operation_id = Uuid::now_v7();
    let mut run = adapter
        .start(StartRequest {
            operation_id,
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            prompt: "Wait".to_owned(),
            model: None,
            mode: ExecutionMode::Normal,
            attachments: Vec::new(),
        })
        .await?;
    assert!(matches!(
        run.events.recv().await,
        Some(ProviderEvent::ConversationStarted { .. })
    ));
    adapter.cancel(operation_id).await?;
    assert_eq!(run.events.recv().await, Some(ProviderEvent::Cancelled));
    Ok(())
}

#[test]
fn remote_cleartext_and_resolved_secret_debug_are_denied_or_redacted()
-> Result<(), Box<dyn std::error::Error>> {
    let result = OpenAiCompatibleProfile::new(
        ProviderAdapterId::new("unsafe-api")?,
        "Unsafe",
        Url::parse("http://example.com/v1/")?,
        OpenAiApiStyle::Responses,
        SecretReference::new(Uuid::nil()),
        "model",
    );
    assert!(result.is_err());
    for endpoint in [
        "https://user:password@example.com/v1/",
        "https://example.com/v1/?api_key=secret-value",
        "https://example.com/v1/#secret-value",
    ] {
        assert!(
            OpenAiCompatibleProfile::new(
                ProviderAdapterId::new("unsafe-api")?,
                "Unsafe",
                Url::parse(endpoint)?,
                OpenAiApiStyle::Responses,
                SecretReference::new(Uuid::nil()),
                "model",
            )
            .is_err(),
            "credential-bearing URL was accepted: {endpoint}"
        );
    }
    assert_eq!(
        format!("{:?}", crate::ResolvedSecret::new("secret-value")),
        "ResolvedSecret([REDACTED])"
    );
    Ok(())
}
