//! Direct provider adapters. Application tools are exchanged as data.
mod codex_auth;
mod discovery;
// RevExt: 1
use std::{fmt, time::Duration};
// RevExt: 2
use anyhow::{Result, bail, ensure};
use reqwest::{Client, RequestBuilder, Response};
use serde::Deserialize;
use serde_json::{Map, Value, json};
// RevExt: 3
use crate::{
    application::management::{self, SettingsRepository},
    domain::{Connection, Method, Model, hidden_codex_model},
};
// RevExt: 4
pub use discovery::discover;
// RevExt: 5
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
const CODEX_URL: &str = "https://chatgpt.com/backend-api/codex";
const MAX_PROVIDER_BYTES: usize = 16 * 1024 * 1024;
// RevExt: 6
#[derive(Debug)]
pub struct ProviderError {
    pub status: u16,
    pub code: &'static str,
    pub message: &'static str,
}  // RevExt: 50
// RevExt: 7
impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }  // RevExt: 103
}  // RevExt: 51
// RevExt: 8
impl std::error::Error for ProviderError {}
// RevExt: 9
fn failure(status: u16, code: &'static str, message: &'static str) -> anyhow::Error {
    ProviderError {
        status,
        code,
        message,
    }  // RevExt: 104
    .into()
}  // RevExt: 52
// RevExt: 10
fn invalid_response() -> anyhow::Error {
    failure(
        502,
        "PROVIDER_RESPONSE_INVALID",
        "The provider returned an invalid response.",
    )
}  // RevExt: 53
// RevExt: 11
pub async fn models(
    client: &Client,
    connection: &Connection,  // RevExt: 88
    key: Option<&str>,
) -> Result<Vec<Model>> {  // RevExt: 92
    match connection.method {
        Method::Codex => {
            let version = discovery::codex_client_version(connection).await;
            let response = codex_request(
                client,
                connection,
                &format!("/models?client_version={version}"),
                None,
            )
            .await?;
            let catalog: CodexCatalog = bounded_json(response).await?;
            Ok(catalog.models.into_iter().filter_map(codex_model).collect())
        }  // RevExt: 139
        Method::Openrouter => {
            let response = send(
                client
                    .get(format!("{OPENROUTER_URL}/models"))
                    .bearer_auth(openrouter_key(key)?),
            )
            .await?;
            let catalog: OpenRouterCatalog = bounded_json(response).await?;
            Ok(catalog
                .data
                .into_iter()
                .filter_map(openrouter_model)
                .collect())
        }  // RevExt: 140
    }  // RevExt: 105
}  // RevExt: 54
// RevExt: 12
pub async fn infer(
    client: &Client,
    connection: &Connection,  // RevExt: 89
    key: Option<&str>,
    request: &Value,
) -> Result<Value> {
    let output = match connection.method {
        Method::Openrouter => {
            let response = send(
                client
                    .post(format!("{OPENROUTER_URL}/chat/completions"))
                    .bearer_auth(openrouter_key(key)?)
                    .json(&openrouter_payload(request)),
            )
            .await?;
            let completion = bounded_json::<Value>(response).await?;
            if let Some(error) = openrouter_body_error(&completion) {
                bail!(upstream_body_failure(error));
            }
            openrouter_output(serde_json::from_value(completion).map_err(|_| invalid_response())?)?
        }  // RevExt: 141
        Method::Codex => {
            let payload = codex_payload(request)?;
            let response = codex_request(client, connection, "/responses", Some(&payload)).await?;
            codex_output(completed_response(response).await?)?
        }  // RevExt: 142
    };
    let mut result = json!({
        "request_id": request["request_id"],
        "content": output.content,
        "tool_calls": output.calls,
        "usage": {"input_tokens": output.input_tokens, "output_tokens": output.output_tokens},
        "provider": match connection.method {
            Method::Codex => "codex",
            Method::Openrouter => "openrouter",
        },
        "method": connection.method,
        "model": request["model"],
    });
    if let Some(effort) = request.get("reasoning_effort") {
        result["reasoning_effort"] = effort.clone();
    }  // RevExt: 106
    Ok(result)
}  // RevExt: 55
// RevExt: 13
fn openrouter_key(key: Option<&str>) -> Result<&str> {
    key.filter(|key| !key.is_empty()).ok_or_else(|| {
        failure(
            401,
            "OPENROUTER_KEY_REQUIRED",
            "Add an OpenRouter key to this connection in Provider Router.",
        )
    })  // RevExt: 94
}  // RevExt: 56
// RevExt: 14
fn openrouter_payload(request: &Value) -> Value {
    let mut payload = json!({
        "model": request["model"],
        "messages": request["messages"],
        "stream": false,
        "provider": {"require_parameters": true},
    });
    if request["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty())
    {
        payload["tools"] = request["tools"].clone();
    }  // RevExt: 107
    if let Some(effort) = request.get("reasoning_effort") {
        payload["reasoning"] = json!({"effort": effort});
    }  // RevExt: 108
    if let Some(maximum) = request.get("max_output_tokens") {
        payload["max_tokens"] = maximum.clone();
    }  // RevExt: 109
    for key in ["response_format", "tool_choice", "parallel_tool_calls"] {
        if let Some(value) = request.get(key) {
            payload[key] = value.clone();
        }  // RevExt: 143
    }  // RevExt: 110
    payload
}  // RevExt: 57
// RevExt: 15
struct Completion {
    content: String,
    calls: Vec<Value>,
    input_tokens: u64,
    output_tokens: u64,
}  // RevExt: 58
// RevExt: 16
#[derive(Deserialize)]
struct OpenRouterCompletion {
    choices: Vec<OpenRouterChoice>,
    usage: OpenRouterUsage,
}  // RevExt: 59
// RevExt: 17
#[derive(Deserialize)]
struct OpenRouterChoice {
    finish_reason: Option<String>,
    message: OpenRouterMessage,
}  // RevExt: 60
// RevExt: 18
#[derive(Deserialize)]
struct OpenRouterMessage {
    content: Option<String>,
    tool_calls: Option<Vec<Value>>,
}  // RevExt: 61
// RevExt: 19
#[derive(Deserialize)]
struct OpenRouterUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}  // RevExt: 62
// RevExt: 20
fn openrouter_output(response: OpenRouterCompletion) -> Result<Completion> {
    let choice = response
        .choices
        .into_iter()
        .next()
        .ok_or_else(invalid_response)?;
    ensure!(
        matches!(choice.finish_reason.as_deref(), Some("stop" | "tool_calls")),
        incomplete()
    );
    Ok(Completion {
        content: choice.message.content.unwrap_or_default(),
        calls: choice.message.tool_calls.unwrap_or_default(),
        input_tokens: response.usage.prompt_tokens,
        output_tokens: response.usage.completion_tokens,
    })  // RevExt: 95
}  // RevExt: 63
// RevExt: 21
fn codex_payload(request: &Value) -> Result<Value> {
    let mut input = Vec::new();
    for message in request["messages"].as_array().into_iter().flatten() {
        let role = message["role"].as_str().unwrap_or_default();
        if role == "tool" {
            let call_id = message["tool_call_id"].as_str().ok_or_else(|| {
                failure(
                    400,
                    "INVALID_HISTORY",
                    "Tool results must identify their tool call.",
                )
            })?;
            input.push(json!({
                "type": "function_call_output", "call_id": call_id,
                "output": text_content(&message["content"]),
            }));
            continue;
        }  // RevExt: 144
        let kind = if role == "assistant" {
            "output_text"
        } else {
            "input_text"
        };
        let content = if let Some(parts) = message["content"].as_array() {
            parts
                .iter()
                .map(|part| match part["type"].as_str() {
                    Some("image_url") => json!({
                        "type": "input_image", "image_url": part["image_url"]["url"]
                    }),
                    _ => json!({"type": kind, "text": part["text"]}),
                })
                .collect::<Vec<_>>()
        } else {
            let text = text_content(&message["content"]);
            if text.is_empty() {
                vec![]
            } else {
                vec![json!({"type": kind, "text": text})]
            }  // RevExt: 153
        };
        if !content.is_empty() {
            let role = if role == "system" { "developer" } else { role };
            input.push(json!({"role": role, "content": content}));
        }  // RevExt: 145
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            input.push(json!({
                "type": "function_call", "call_id": call["id"],
                "name": call["function"]["name"], "arguments": call["function"]["arguments"],
            }));
        }  // RevExt: 146
    }  // RevExt: 111
// RevExt: 22
    let tools: Vec<Value> = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|tool| {
            let mut function = tool["function"].clone();
            function["type"] = json!("function");
            function
        })  // RevExt: 131
        .collect();
    let mut payload = json!({
        "model": request["model"], "instructions": "", "input": input,
        "store": false, "stream": true, "tools": tools,
    });
    if let Some(effort) = request.get("reasoning_effort") {
        payload["reasoning"] = json!({"effort": effort});
    }  // RevExt: 112
    if let Some(choice) = request.get("tool_choice") {
        payload["tool_choice"] = if choice.is_object() {
            json!({"type": "function", "name": choice["function"]["name"]})
        } else {
            choice.clone()
        };
    }  // RevExt: 113
    if let Some(parallel) = request.get("parallel_tool_calls") {
        payload["parallel_tool_calls"] = parallel.clone();
    }  // RevExt: 114
    // Codex controls the subscription output budget. The API validates the completed usage.
    if let Some(format) = request["response_format"]["json_schema"].as_object() {
        let mut format = format.clone();
        format.insert("type".into(), json!("json_schema"));
        payload["text"] = json!({"format": format});
    }  // RevExt: 115
    Ok(payload)
}  // RevExt: 64
// RevExt: 23
fn text_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }  // RevExt: 116
}  // RevExt: 65
// RevExt: 24
async fn codex_request(
    client: &Client,
    connection: &Connection,  // RevExt: 90
    route: &str,
    payload: Option<&Value>,
) -> Result<Response> {
    let home = codex_auth::CredentialHome::for_connection(connection).await?;
    let mut rejected = None;
    for retry in [false, true] {
        let credentials = home.authorize(client, rejected.as_deref()).await?;
        let url = format!("{CODEX_URL}{route}");
        let request = match payload {
            Some(body) => client.post(&url).json(body),
            None => client.get(&url),
        };
        let mut request = request
            .bearer_auth(&credentials.access_token)
            .header("Accept", "text/event-stream")
            .header("OpenAI-Beta", "responses=experimental")
            .header("originator", "provider_router")
            .header(
                "User-Agent",
                concat!("provider-router/", env!("CARGO_PKG_VERSION")),
            );
        if let Some(account) = credentials.account_id {
            request = request.header("ChatGPT-Account-ID", account);
        }  // RevExt: 147
        let response = request.send().await.map_err(|_| unavailable())?;
        if response.status().as_u16() == 401 && !retry {
            rejected = Some(credentials.access_token);
            continue;
        }  // RevExt: 148
        return check_status(response);
    }  // RevExt: 117
    unreachable!("the final Codex attempt returns a response or error")
}  // RevExt: 66
// RevExt: 25
async fn send(request: RequestBuilder) -> Result<Response> {
    check_status(request.send().await.map_err(|_| unavailable())?)
}  // RevExt: 67
// RevExt: 26
fn check_status(response: Response) -> Result<Response> {
    match response.status().as_u16() {
        200..=299 => Ok(response),
        status => Err(status_failure(status)),
    }
}

/// OpenRouter reports failures after its headers inside an HTTP 200 body, possibly without completion fields.
fn openrouter_body_error(completion: &Value) -> Option<&Value> {
    [&completion["error"], &completion["choices"][0]["error"]]
        .into_iter()
        .find(|error| !error.is_null())
}

/// An in-body provider error carries the HTTP status the provider would have returned.
fn upstream_body_failure(error: &Value) -> anyhow::Error {
    match error["code"].as_u64().and_then(|code| u16::try_from(code).ok()) {
        Some(status @ 400..=599) => status_failure(status),
        _ => incomplete(),
    }
}

fn status_failure(status: u16) -> anyhow::Error {
    match status {
        400 | 413 | 422 => failure(
            400,
            "PROVIDER_REQUEST_REJECTED",
            "The provider rejected the request as invalid.",
        ),  // RevExt: 159
        429 => failure(
            429,  // RevExt: 176
            "PROVIDER_RATE_LIMIT",  // RevExt: 178
            "The provider is currently rate limited.",  // RevExt: 180
        ),  // RevExt: 160
        401 => failure(
            502,
            "PROVIDER_AUTH_REQUIRED",
            "The provider rejected the credential. Check the connection in Provider Router.",
        ),  // RevExt: 161
        _ => unavailable(),
    }  // RevExt: 118
}  // RevExt: 68
// RevExt: 27
fn unavailable() -> anyhow::Error {
    failure(
        502,
        "PROVIDER_UNAVAILABLE",
        "The provider request failed. Check the connection in Provider Router.",
    )
}  // RevExt: 69
// RevExt: 28
async fn bounded_json<T: serde::de::DeserializeOwned>(mut response: Response) -> Result<T> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        ensure!(
            bytes.len() + chunk.len() <= MAX_PROVIDER_BYTES,
            output_limit()
        );
        bytes.extend_from_slice(&chunk);
    }  // RevExt: 119
    serde_json::from_slice(&bytes).map_err(|_| invalid_response())
}  // RevExt: 70
// RevExt: 29
fn output_limit() -> anyhow::Error {
    failure(
        502,
        "PROVIDER_OUTPUT_LIMIT",
        "The provider response exceeded the supported size.",
    )
}  // RevExt: 71
// RevExt: 30
/// Streamed upstream failures keep the meaning of the equivalent HTTP status.
fn stream_failure(error: &Value) -> anyhow::Error {
    match error["code"].as_str() {
        Some("server_error") => unavailable(),
        Some("rate_limit_exceeded") => failure(
            429,  // RevExt: 177
            "PROVIDER_RATE_LIMIT",  // RevExt: 179
            "The provider is currently rate limited.",  // RevExt: 181
        ),
        _ => incomplete(),
    }  // RevExt: 173
}  // RevExt: 172
// RevExt: 170
fn incomplete() -> anyhow::Error {
    failure(
        502,
        "INCOMPLETE_COMPLETION",
        "The provider did not finish the response.",
    )
}  // RevExt: 72
// RevExt: 31
async fn completed_response(mut response: Response) -> Result<Value> {
    let mut decoder = CodexSse::default();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if let Some(completion) = decoder.feed(&chunk)? {
            return Ok(completion);
        }  // RevExt: 149
    }  // RevExt: 120
    bail!(incomplete())
}  // RevExt: 73
// RevExt: 32
/// Framing stays byte-oriented until all lines of the SSE event have arrived.
#[derive(Default)]
struct CodexSse {
    line: Vec<u8>,
    data: Vec<u8>,
    received: usize,
}  // RevExt: 74
// RevExt: 33
impl CodexSse {
    fn feed(&mut self, chunk: &[u8]) -> Result<Option<Value>> {
        self.received += chunk.len();
        ensure!(self.received <= MAX_PROVIDER_BYTES, output_limit());
        for line in chunk.split_inclusive(|byte| *byte == b'\n') {
            self.line.extend_from_slice(line);
            if !line.ends_with(b"\n") {
                continue;
            }  // RevExt: 154
            let line = self.line.strip_suffix(b"\n").unwrap_or_default();
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() {
                if !self.data.is_empty() && self.data != b"[DONE]" {
                    let event: Value =
                        serde_json::from_slice(&self.data).map_err(|_| invalid_response())?;
                    match event["type"].as_str() {
                        Some("response.completed") if event["response"].is_object() => {
                            return Ok(Some(event["response"].clone()));
                        }  // RevExt: 182
                        Some("response.failed") => bail!(stream_failure(&event["response"]["error"])),
                        Some("error") => bail!(stream_failure(&event)),
                        Some("response.incomplete") => bail!(incomplete()),
                        _ => {}
                    }
                }
                self.data.clear();
            } else if let Some(data) = line.strip_prefix(b"data:") {
                if !self.data.is_empty() {
                    self.data.push(b'\n');
                }
                self.data
                    .extend_from_slice(data.strip_prefix(b" ").unwrap_or(data));
            }  // RevExt: 155
            self.line.clear();
        }  // RevExt: 150
        Ok(None)
    }  // RevExt: 121
}  // RevExt: 75
// RevExt: 34
fn codex_output(response: Value) -> Result<Completion> {
    ensure!(response["status"] == "completed", incomplete());
    let mut calls = Vec::new();
    let mut content = String::new();
    for item in response["output"].as_array().ok_or_else(invalid_response)? {
        match item["type"].as_str() {
            Some("function_call") => {
                let id = item["call_id"].as_str().ok_or_else(invalid_response)?;
                let name = item["name"].as_str().ok_or_else(invalid_response)?;
                let arguments = item["arguments"].as_str().ok_or_else(invalid_response)?;
                calls.push(json!({
                    "id": id, "type": "function", "function": {"name": name, "arguments": arguments},
                }));
            }  // RevExt: 156
            Some("message") => {
                for part in item["content"].as_array().ok_or_else(invalid_response)? {
                    match part["type"].as_str() {
                        Some("output_text") => {
                            content.push_str(part["text"].as_str().ok_or_else(invalid_response)?);
                        }  // RevExt: 183
                        Some("refusal") => bail!(failure(
                            422,
                            "PROVIDER_REFUSAL",
                            "The provider declined this request."
                        )),
                        _ => {}
                    }
                }
            }  // RevExt: 157
            Some("reasoning") => {}
            _ => bail!(failure(
                502,
                "UNSUPPORTED_PROVIDER_OUTPUT",
                "The subscription returned an unsupported output item."
            )),
        }  // RevExt: 151
    }  // RevExt: 122
    Ok(Completion {
        content,
        calls,
        input_tokens: response["usage"]["input_tokens"]
            .as_u64()
            .ok_or_else(invalid_response)?,
        output_tokens: response["usage"]["output_tokens"]
            .as_u64()
            .ok_or_else(invalid_response)?,
    })  // RevExt: 96
}  // RevExt: 76
// RevExt: 35
#[derive(Deserialize)]
struct CodexCatalog {
    models: Vec<CodexModel>,
}  // RevExt: 77
// RevExt: 36
#[derive(Deserialize)]
struct CodexModel {
    slug: String,
    display_name: Option<String>,
    context_window: Option<u64>,
    max_output_tokens: Option<u64>,
    default_reasoning_level: Option<String>,
    #[serde(default)]
    supported_reasoning_levels: Vec<Value>,
    #[serde(default)]
    input_modalities: Vec<String>,
}  // RevExt: 78
// RevExt: 37
fn codex_model(entry: CodexModel) -> Option<Model> {
    let context = entry.context_window.filter(|maximum| *maximum > 0)?;
    if entry.slug.is_empty() || hidden_codex_model(&entry.slug) {
        return None;
    }  // RevExt: 123
    let efforts: Vec<String> = entry
        .supported_reasoning_levels
        .iter()
        .filter_map(|value| {
            value
                .as_str()
                .or_else(|| value["effort"].as_str())
                .map(str::to_owned)
        })  // RevExt: 132
        .collect();
    Some(Model {
        name: entry.display_name.unwrap_or_else(|| entry.slug.clone()),
        id: entry.slug,
        default_reasoning_effort: entry
            .default_reasoning_level
            .filter(|effort| efforts.contains(effort)),
        reasoning_efforts: efforts,
        context_window: context,
        max_output_tokens: entry
            .max_output_tokens
            .filter(|value| *value > 0)
            .unwrap_or((context / 5).max(1)),
        tools: true,
        structured_output: true,
        input_modalities: modalities(&entry.input_modalities),
    })  // RevExt: 97
}  // RevExt: 79
// RevExt: 38
#[derive(Deserialize)]
struct OpenRouterCatalog {
    data: Vec<OpenRouterModel>,
}  // RevExt: 80
// RevExt: 39
#[derive(Deserialize)]
struct OpenRouterModel {
    id: String,
    name: String,
    context_length: Option<u64>,
    top_provider: Option<OpenRouterMaximum>,
    #[serde(default)]
    supported_parameters: Vec<String>,
    architecture: Option<OpenRouterArchitecture>,
    reasoning: Option<Map<String, Value>>,
}  // RevExt: 81
// RevExt: 40
#[derive(Deserialize)]
struct OpenRouterMaximum {
    max_completion_tokens: Option<u64>,
}  // RevExt: 82
// RevExt: 41
#[derive(Default, Deserialize)]
struct OpenRouterArchitecture {
    #[serde(default)]
    input_modalities: Vec<String>,
}  // RevExt: 83
// RevExt: 42
fn openrouter_model(entry: OpenRouterModel) -> Option<Model> {
    let context = entry.context_length.filter(|value| *value > 0)?;
    let reasoning = entry.reasoning.unwrap_or_default();
    let mut efforts: Vec<String> = match reasoning.get("supported_efforts") {
        Some(Value::Null) => ["max", "xhigh", "high", "medium", "low", "minimal", "none"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        Some(Value::Array(efforts)) => efforts
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => vec![],
    };
    if reasoning.get("mandatory") == Some(&Value::Bool(true)) {
        efforts.retain(|effort| effort != "none");
    }  // RevExt: 124
    Some(Model {
        id: entry.id,
        name: entry.name,
        default_reasoning_effort: reasoning
            .get("default_effort")
            .and_then(Value::as_str)
            .filter(|effort| efforts.iter().any(|allowed| allowed == *effort))
            .map(str::to_owned),
        reasoning_efforts: efforts,
        context_window: context,
        max_output_tokens: entry
            .top_provider
            .and_then(|value| value.max_completion_tokens)
            .filter(|value| *value > 0)
            .unwrap_or((context / 5).max(1)),
        tools: entry
            .supported_parameters
            .iter()
            .any(|value| value == "tools"),
        structured_output: entry
            .supported_parameters
            .iter()
            .any(|value| value == "structured_outputs"),
        input_modalities: modalities(&entry.architecture.unwrap_or_default().input_modalities),
    })  // RevExt: 98
}  // RevExt: 84
// RevExt: 43
fn modalities(input: &[String]) -> Vec<String> {
    let mut output = vec!["text".into()];
    if input.iter().any(|value| value == "image") {
        output.push("image".into());
    }  // RevExt: 125
    output
}  // RevExt: 85
// RevExt: 44
pub struct Catalog(reqwest::Client);
// RevExt: 100
impl Catalog {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self(client))
    }  // RevExt: 129
// RevExt: 101
    pub async fn models(
        &self,
        store: &impl SettingsRepository,
        connection: &Connection,
    ) -> Result<Vec<Model>> {
        management::catalog(store, connection, async |connection, key| {
            models(&self.0, connection, key).await
        })  // RevExt: 133
        .await
    }  // RevExt: 130
}  // RevExt: 102
// RevExt: 134
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_catalog_skips_internal_models() {
        let catalog: CodexCatalog = serde_json::from_value(json!({"models": [
            {"slug": "gpt-6-sol", "display_name": "GPT-6-Sol", "context_window": 272000},
            {"slug": "codex-auto-review", "display_name": "Codex Auto Review", "context_window": 272000},
            {"slug": "gpt-reserve", "display_name": "GPT-Reserve", "context_window": 32000}
        ]}))
        .unwrap();
        let ids: Vec<String> = catalog.models.into_iter().filter_map(codex_model).map(|model| model.id).collect();
        assert_eq!(ids, ["gpt-6-sol"]);
    }
// RevExt: 45
    #[test]  // RevExt: 162
    fn codex_stream_preserves_split_unicode_and_rejects_incomplete_completion() {
        let event = "event: response.completed\r\ndata: {\"type\":\"response.completed\",\r\ndata: \"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"valid 🌿\"}]}],\"usage\":{\"input_tokens\":2,\"output_tokens\":3}}}\r\n\r\n";
        let mut decoder = CodexSse::default();
        let output = event
            .as_bytes()
            .chunks(3)
            .find_map(|chunk| decoder.feed(chunk).unwrap())
            .expect("complete stream");
        let completion = codex_output(output).unwrap();
        assert_eq!(completion.content, "valid 🌿");
        assert_eq!(completion.output_tokens, 3);
        assert!(
            CodexSse::default()
                .feed(b"data: {\"type\":\"response.incomplete\"}\n\n")
                .is_err()
        );
    }  // RevExt: 174
// RevExt: 171
    #[test]
    fn openrouter_body_errors_keep_upstream_status_meaning() {
        let cases = [
            (json!({"code": 502, "message": "Provider disconnected"}), 502, "PROVIDER_UNAVAILABLE"),
            (json!({"code": 402, "message": "Insufficient credits"}), 502, "PROVIDER_UNAVAILABLE"),
            (json!({"code": 429, "message": "Rate limited"}), 429, "PROVIDER_RATE_LIMIT"),
            (json!({"code": 400, "message": "Bad request"}), 400, "PROVIDER_REQUEST_REJECTED"),
            (json!({"message": "Unknown"}), 502, "INCOMPLETE_COMPLETION"),
        ];
        for (error, status, code) in cases {
            let top_level = upstream_body_failure(&error).downcast::<ProviderError>().unwrap();
            assert_eq!((top_level.status, top_level.code), (status, code));
            // OpenRouter's documented mid-generation failure has partial output and no usage.
            let choice = json!({
                "choices": [{"finish_reason": "error", "message": {"content": "partial"}, "error": error}],
            });
            let in_choice = upstream_body_failure(openrouter_body_error(&choice).unwrap())
                .downcast::<ProviderError>()
                .unwrap();
            assert_eq!((in_choice.status, in_choice.code), (status, code));
        }
        let completion = json!({
            "error": null,
            "choices": [{"finish_reason": "stop", "message": {"content": "done"}, "error": null}],
        });
        assert!(openrouter_body_error(&completion).is_none());
    }

    #[test]  // RevExt: 184
    fn codex_stream_failures_keep_upstream_availability_meaning() {
        let cases: [(&[u8], u16, &str); 5] = [
            (b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\"}}}\n\n", 502, "PROVIDER_UNAVAILABLE"),
            (b"data: {\"type\":\"error\",\"code\":\"server_error\"}\n\n", 502, "PROVIDER_UNAVAILABLE"),
            (b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"rate_limit_exceeded\"}}}\n\n", 429, "PROVIDER_RATE_LIMIT"),
            (b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"invalid_prompt\"}}}\n\n", 502, "INCOMPLETE_COMPLETION"),
            (b"data: {\"type\":\"response.incomplete\"}\n\n", 502, "INCOMPLETE_COMPLETION"),
        ];  // RevExt: 185
        for (event, status, code) in cases {
            let error = CodexSse::default()
                .feed(event)
                .unwrap_err()  // RevExt: 187
                .downcast::<ProviderError>()  // RevExt: 189
                .unwrap();  // RevExt: 191
            assert_eq!((error.status, error.code), (status, code));
        }  // RevExt: 175
    }  // RevExt: 126
// RevExt: 46
    #[test]  // RevExt: 163
    fn codex_payload_carries_images_tool_calls_and_structured_response() {
        let request = json!({
            "model": "codex-test",
            "messages": [
                {"role":"system","content":"instructions"},
                {"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]},
                {"role":"assistant","content":null,"tool_calls":[{"id":"call_test","type":"function","function":{"name":"lookup","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"call_test","content":"result"},
            ],
            "max_output_tokens": 15,
            "response_format": {"type":"json_schema","json_schema":{"name":"result","schema":{"type":"object"}}},
        });
        let payload = codex_payload(&request).unwrap();
        assert_eq!(payload["input"][0]["role"], "developer");
        assert_eq!(
            payload["input"][1]["content"][0]["image_url"],
            "data:image/png;base64,AA=="
        );
        assert_eq!(payload["input"][2]["name"], "lookup");
        assert_eq!(payload["input"][3]["output"], "result");
        assert_eq!(payload["text"]["format"]["name"], "result");
        assert!(payload.get("max_output_tokens").is_none());
        assert_eq!(payload["store"], false);
    }  // RevExt: 127
// RevExt: 47
    #[test]  // RevExt: 164
    fn openrouter_preserves_tools_reasoning_and_usage() {
        let tool =
            json!({"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}});
        let request = json!({
            "model":"test/model",
            "messages":[{"role":"user","content":"Find the entry."}],
            "tools":[tool],
            "tool_choice":"required",
            "reasoning_effort":"low",
            "max_output_tokens":120,
        });
        let payload = openrouter_payload(&request);
        assert_eq!(payload["tools"][0], tool);
        assert_eq!(payload["tool_choice"], "required");
        assert_eq!(payload["reasoning"]["effort"], "low");
        assert_eq!(payload["max_tokens"], 120);
// RevExt: 48
        let tool_call = json!({"type":"function","id":"call_test","function":{"name":"lookup","arguments":"{}"}});
        let mut completion = json!({
            "choices":[{"finish_reason":"tool_calls","message":{"tool_calls":[tool_call]}}],
            "usage":{"prompt_tokens":14,"completion_tokens":3},
        });
        let accepted =
            openrouter_output(serde_json::from_value(completion.clone()).unwrap()).unwrap();
        assert_eq!(accepted.calls, [tool_call]);
        assert_eq!((accepted.input_tokens, accepted.output_tokens), (14, 3));
        completion["choices"][0]["finish_reason"] = "length".into();
        assert!(openrouter_output(serde_json::from_value(completion).unwrap()).is_err());
    }  // RevExt: 128
// RevExt: 135
    #[test]  // RevExt: 165
    fn upstream_statuses_distinguish_request_rejections_from_provider_failures() {
        let cases = [
            (400, 400, "PROVIDER_REQUEST_REJECTED"),
            (413, 400, "PROVIDER_REQUEST_REJECTED"),
            (422, 400, "PROVIDER_REQUEST_REJECTED"),
            (401, 502, "PROVIDER_AUTH_REQUIRED"),
            (403, 502, "PROVIDER_UNAVAILABLE"),
            (404, 502, "PROVIDER_UNAVAILABLE"),
            (429, 429, "PROVIDER_RATE_LIMIT"),
            (500, 502, "PROVIDER_UNAVAILABLE"),
            (503, 502, "PROVIDER_UNAVAILABLE"),
        ];  // RevExt: 186
        for (upstream, status, code) in cases {
            let response = axum::http::Response::builder()
                .status(upstream)
                .body("upstream-private-detail")
                .unwrap();  // RevExt: 166
            let error = check_status(response.into())
                .unwrap_err()  // RevExt: 188
                .downcast::<ProviderError>()  // RevExt: 190
                .unwrap();  // RevExt: 167
            assert_eq!((error.status, error.code), (status, code), "HTTP {upstream}");
            if status == 400 {
                assert_eq!(error.message, "The provider rejected the request as invalid.");
            }  // RevExt: 158
        }  // RevExt: 152
    }  // RevExt: 137
// RevExt: 136
    #[tokio::test]
    async fn upstream_transport_failure_remains_unavailable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let disconnect = tokio::spawn(async move { drop(listener.accept().await.unwrap()) });
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();  // RevExt: 168
        let error = send(client.post(format!("http://{address}")))
            .await
            .unwrap_err()
            .downcast::<ProviderError>()
            .unwrap();  // RevExt: 169
        assert_eq!((error.status, error.code), (502, "PROVIDER_UNAVAILABLE"));
        disconnect.await.unwrap();
    }  // RevExt: 138
}  // RevExt: 86
// RevExt: 49
