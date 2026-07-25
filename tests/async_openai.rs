use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::Arc;

use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::CreateChatCompletionRequest;
use futures::StreamExt;
use no_llm_api::config::TokenizerSettings;
use no_llm_api::dataset::{ConversationScripts, ensure_sample_dataset};
use no_llm_api::http::build_router;
use no_llm_api::model::{
    ChatCompletionRequest, ChatCompletionRequestMessage, ChatCompletionResponse,
    ChatCompletionStreamOptions, ChatRole, FinishReason, MessageContent,
};
use no_llm_api::service::ChatService;
use no_llm_api::tokenizer::load;
use tempfile::tempdir;
use tokio::net::TcpListener;

const ENV_FLAG: &str = "ASYNC_OPENAI_COMPAT";

#[tokio::test]
async fn async_openai_compatibility() -> anyhow::Result<()> {
    if std::env::var(ENV_FLAG).is_err() {
        eprintln!(
            "skipping async-openai compatibility test; set {}=1 to enable",
            ENV_FLAG
        );
        return Ok(());
    }

    let tmp = tempdir()?;
    let dataset_path = tmp.path().join("sample.parquet");
    ensure_sample_dataset(&dataset_path)?;

    let tokenizer = load(&TokenizerSettings {
        preset: "cl100k_base".to_string(),
    })?;
    let scripts = ConversationScripts::load(&dataset_path, &tokenizer)?;
    let service = Arc::new(ChatService::new(
        scripts,
        tokenizer.clone(),
        NonZeroU32::new(30).unwrap(),
    ));
    let app = build_router(service);

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await
            .unwrap();
    });

    let client = client_for(addr);

    // Non-streamed completion
    let non_stream = request_with_text("Summarize the sprint update.");
    let response = client.chat().create(convert_request(&non_stream)?).await?;
    let ours: ChatCompletionResponse = serde_json::from_value(serde_json::to_value(&response)?)?;
    let text = match &ours.choices[0].message.content {
        Some(MessageContent::Text(text)) => text.as_str(),
        _ => "",
    };
    assert!(text.contains("Sprint closed"));

    // Streaming completion with usage
    let mut streaming = request_with_text("Summarize the sprint update.");
    streaming.stream = true;
    streaming.stream_options = Some(ChatCompletionStreamOptions {
        include_usage: true,
    });
    let mut stream = client
        .chat()
        .create_stream(convert_request(&streaming)?)
        .await?;
    let mut saw_usage = false;
    while let Some(event) = stream.next().await {
        let chunk = event?;
        if chunk.usage.is_some() {
            saw_usage = true;
        }
    }
    assert!(saw_usage);

    // Tool-call scenario
    let tool_request = request_with_text("Help me unblock a failing integration test.");
    let tool_response = client
        .chat()
        .create(convert_request(&tool_request)?)
        .await?;
    let tool_response: ChatCompletionResponse =
        serde_json::from_value(serde_json::to_value(&tool_response)?)?;
    let finish = tool_response
        .choices
        .first()
        .and_then(|choice| choice.finish_reason.clone())
        .unwrap_or(FinishReason::Stop);
    assert_eq!(finish, FinishReason::ToolCalls);

    let _ = tx.send(());
    Ok(())
}

fn client_for(addr: SocketAddr) -> Client<OpenAIConfig> {
    let config = OpenAIConfig::new()
        .with_api_base(format!("http://{}", addr))
        .with_api_key("test".to_string());
    Client::with_config(config)
}

fn request_with_text(text: &str) -> ChatCompletionRequest {
    ChatCompletionRequest {
        model: "test-model".to_string(),
        messages: vec![ChatCompletionRequestMessage {
            role: ChatRole::User,
            content: Some(MessageContent::Text(text.to_string())),
            name: None,
            tool_calls: None,
            function_call: None,
            audio: None,
            refusal: None,
        }],
        ..Default::default()
    }
}

fn convert_request(
    request: &ChatCompletionRequest,
) -> Result<CreateChatCompletionRequest, serde_json::Error> {
    let value = serde_json::to_value(request)?;
    serde_json::from_value(value)
}
