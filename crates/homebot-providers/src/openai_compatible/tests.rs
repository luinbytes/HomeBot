use super::*;
use crate::{ProviderTool, ProviderToolResult};
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
            working_directory: None,
            mode: ExecutionMode::Normal,
            attachments: Vec::new(),
            tools: Vec::new(),
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
            working_directory: None,
            mode: ExecutionMode::Normal,
            attachments: Vec::new(),
            tools: Vec::new(),
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

#[tokio::test]
async fn responses_tools_continue_only_after_homebot_returns_the_result()
-> Result<(), Box<dyn std::error::Error>> {
    let requests = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/v1/responses",
        post({
            let requests = Arc::clone(&requests);
            move |Json(body): Json<Value>| {
                let requests = Arc::clone(&requests);
                async move {
                    let request = requests.fetch_add(1, Ordering::Relaxed);
                    tool_fixture_response(request, &body)
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let adapter = OpenAiCompatibleAdapter::new(
        OpenAiCompatibleProfile::new(
            ProviderAdapterId::new("openai-tools")?,
            "Fixture API",
            Url::parse(&format!("http://{address}/v1/"))?,
            OpenAiApiStyle::Responses,
            SecretReference::new(Uuid::now_v7()),
            "model-fixture",
        )?,
        Arc::new(TestSecrets {
            value: "fixture-secret".to_owned(),
            resolutions: AtomicUsize::new(0),
        }),
    )?;
    let mut run = adapter
        .start(StartRequest {
            operation_id: Uuid::now_v7(),
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            prompt: "Coordinate".to_owned(),
            model: None,
            working_directory: None,
            mode: ExecutionMode::Normal,
            attachments: Vec::new(),
            tools: vec![ProviderTool {
                name: "send_bot_message".to_owned(),
                description: "Send a direct Bot message".to_owned(),
                input_schema: json!({"type":"object"}),
            }],
        })
        .await?;
    assert!(matches!(
        run.events.recv().await,
        Some(ProviderEvent::ConversationStarted { .. })
    ));
    let call = loop {
        match run.events.recv().await {
            Some(ProviderEvent::ToolCall { call }) => break call,
            Some(ProviderEvent::Usage { .. } | ProviderEvent::Activity { .. }) => {}
            other => panic!("expected tool call, got {other:?}"),
        }
    };
    assert_eq!(call.call_id, "call_1");
    assert_eq!(call.arguments["bot_id"], "bot-2");
    adapter
        .resolve_tool_call(
            call.call_id,
            ProviderToolResult {
                success: true,
                content: "Delivered".to_owned(),
            },
        )
        .await?;
    let mut text = String::new();
    loop {
        match run.events.recv().await {
            Some(ProviderEvent::ContentDelta { text: delta }) => text.push_str(&delta),
            Some(ProviderEvent::Usage { .. }) => {}
            Some(ProviderEvent::Completed) => break,
            other => panic!("unexpected provider event: {other:?}"),
        }
    }
    assert_eq!(text, "Verified");
    assert_eq!(requests.load(Ordering::Relaxed), 2);
    Ok(())
}

fn tool_fixture_response(request: usize, body: &Value) -> Response<Body> {
    let stream = if request == 0 {
        assert_eq!(body["tools"][0]["name"], "send_bot_message");
        assert_eq!(body["tools"][0]["parameters"]["type"], "object");
        concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_tools\"}}\n\n",
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"send_bot_message\",\"arguments\":\"{\\\"bot_id\\\":\\\"bot-2\\\",\\\"message\\\":\\\"Please verify\\\"}\"}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_tools\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n"
        )
    } else {
        assert_eq!(body["previous_response_id"], "resp_tools");
        assert_eq!(body["input"][0]["type"], "function_call_output");
        assert_eq!(body["input"][0]["call_id"], "call_1");
        let output: Value =
            serde_json::from_str(body["input"][0]["output"].as_str().unwrap_or_default())
                .unwrap_or(Value::Null);
        assert_eq!(output["success"], true);
        assert_eq!(output["content"], "Delivered");
        concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_done\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Verified\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_done\",\"usage\":{\"input_tokens\":2,\"output_tokens\":1}}}\n\n"
        )
    };
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(stream))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

#[tokio::test]
async fn chat_completions_reassembles_tool_arguments_and_continues()
-> Result<(), Box<dyn std::error::Error>> {
    let requests = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/v1/chat/completions",
        post({
            let requests = Arc::clone(&requests);
            move |Json(body): Json<Value>| {
                let request = requests.fetch_add(1, Ordering::Relaxed);
                async move { chat_tool_fixture_response(request, &body) }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let adapter = OpenAiCompatibleAdapter::new(
        OpenAiCompatibleProfile::new(
            ProviderAdapterId::new("chat-tools")?,
            "Fixture Chat API",
            Url::parse(&format!("http://{address}/v1/"))?,
            OpenAiApiStyle::ChatCompletions,
            SecretReference::new(Uuid::now_v7()),
            "model-fixture",
        )?,
        Arc::new(TestSecrets {
            value: "fixture-secret".to_owned(),
            resolutions: AtomicUsize::new(0),
        }),
    )?;
    let mut run = adapter
        .start(StartRequest {
            operation_id: Uuid::now_v7(),
            bot_id: Uuid::now_v7(),
            chat_id: Uuid::now_v7(),
            prompt: "Coordinate".to_owned(),
            model: None,
            working_directory: None,
            mode: ExecutionMode::Normal,
            attachments: Vec::new(),
            tools: vec![ProviderTool {
                name: "send_bot_message".to_owned(),
                description: "Send a direct Bot message".to_owned(),
                input_schema: json!({"type":"object"}),
            }],
        })
        .await?;
    assert!(matches!(
        run.events.recv().await,
        Some(ProviderEvent::ConversationStarted { .. })
    ));
    let call = loop {
        match run.events.recv().await {
            Some(ProviderEvent::ToolCall { call }) => break call,
            Some(ProviderEvent::Activity { .. } | ProviderEvent::Usage { .. }) => {}
            other => panic!("expected chat tool call, got {other:?}"),
        }
    };
    assert_eq!(call.arguments["bot_id"], "bot-2");
    adapter
        .resolve_tool_call(
            call.call_id,
            ProviderToolResult {
                success: true,
                content: "Delivered".to_owned(),
            },
        )
        .await?;
    assert_eq!(
        run.events.recv().await,
        Some(ProviderEvent::ContentDelta {
            text: "Verified".to_owned()
        })
    );
    assert_eq!(run.events.recv().await, Some(ProviderEvent::Completed));
    assert_eq!(requests.load(Ordering::Relaxed), 2);
    Ok(())
}

fn chat_tool_fixture_response(request: usize, body: &Value) -> Response<Body> {
    let stream = if request == 0 {
        assert_eq!(body["tools"][0]["function"]["name"], "send_bot_message");
        concat!(
            "data: {\"id\":\"chat_tools\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_chat\",\"type\":\"function\",\"function\":{\"name\":\"send_bot_message\",\"arguments\":\"{\\\"bot_id\\\":\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chat_tools\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"bot-2\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chat_tools\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n"
        )
    } else {
        assert_eq!(body["messages"][1]["tool_calls"][0]["id"], "call_chat");
        assert_eq!(body["messages"][2]["role"], "tool");
        concat!(
            "data: {\"id\":\"chat_done\",\"choices\":[{\"delta\":{\"content\":\"Verified\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chat_done\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n"
        )
    };
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(stream))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}
