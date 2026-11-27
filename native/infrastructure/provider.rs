//! Direct provider adapters. Application tools are exchanged as data.
mod codex_auth;
mod discovery;
use crate::{
    application::management::{self, SettingsRepository},
    domain::{Connection, Method, Model, hidden_codex_model},
};
use anyhow::{Result, bail, ensure};
pub use discovery::discover;
use reqwest::{Client, RequestBuilder, Response};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, fmt, time::Duration};
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
const CODEX_URL: &str = "https://chatgpt.com/backend-api/codex";
const MAX_PROVIDER_BYTES: usize = 16 * 1024 * 1024;
#[derive(Debug)]
pub struct ProviderError {
    pub status: u16,
    pub code: &'static str,
    pub message: &'static str,
}
impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}
impl std::error::Error for ProviderError {}
fn failure(status: u16, code: &'static str, message: &'static str) -> anyhow::Error {
    ProviderError {
        status,
        code,
        message,
    }
    .into()
}
fn invalid_response() -> anyhow::Error {
    failure(
        502,
        "PROVIDER_RESPONSE_INVALID",
        "The provider returned an invalid response.",
    )
}
pub async fn models(
    client: &Client,
    connection: &Connection,
    key: Option<&str>,
) -> Result<Vec<Model>> {
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
        }
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
        }
    }
}
pub async fn infer(
    client: &Client,
    connection: &Connection,
    key: Option<&str>,
    request: &Value,
) -> Result<Value> {
    let output = match connection.method {
        Method::Openrouter => {
            let key = openrouter_key(key)?;
            let payload = openrouter_payload(request)?;
            let response = send(
                client
                    .post(format!("{OPENROUTER_URL}/chat/completions"))
                    .bearer_auth(key)
                    .header("X-OpenRouter-Metadata", "enabled")
                    .json(&payload),
            )
            .await?;
            let completion = bounded_json::<Value>(response).await?;
            if let Some(error) = openrouter_body_error(&completion) {
                bail!(upstream_body_failure(error));
            }
            let completion: OpenRouterCompletion =
                serde_json::from_value(completion).map_err(|_| invalid_response())?;
            let mut output = openrouter_output(&completion)?;
            if let Some(state) = &mut output.state {
                let upstream = match payload["provider"]["only"][0].as_str() {
                    Some(tag) => Some(tag.to_owned()),
                    None => match (completion.model.as_deref(), completion.selected_provider()) {
                        (Some(model), Some(provider)) => {
                            openrouter_endpoint_tag(client, key, model, provider).await
                        }
                        _ => None,
                    },
                };
                if let Some(tag) = upstream {
                    state["upstream"] = json!(tag);
                } else {
                    output.state = None;
                }
            }
            output
        }
        Method::Codex => {
            let payload = codex_payload(request)?;
            let response = codex_request(client, connection, "/responses", Some(&payload)).await?;
            codex_output(completed_response(response).await?)?
        }
    };
    let mut result = json!({
        "request_id": request["request_id"],
        "content": output.content,
        "tool_calls": output.calls,
        "usage": {"input_tokens": output.input_tokens, "output_tokens": output.output_tokens,
                  "cached_input_tokens": output.cache_read, "cache_write_tokens": output.cache_write},
        "provider": match connection.method {
            Method::Codex => "codex",
            Method::Openrouter => "openrouter",
        },
        "method": connection.method,
        "model": request["model"],
    });
    if let Some(state) = output.state {
        result["continuation"] =
            json!({"connection_id": connection.id, "model": request["model"], "state": state});
    }
    if let Some(effort) = request.get("reasoning_effort") {
        result["reasoning_effort"] = effort.clone();
    }
    Ok(result)
}
fn openrouter_key(key: Option<&str>) -> Result<&str> {
    key.filter(|key| !key.is_empty()).ok_or_else(|| {
        failure(
            401,
            "OPENROUTER_KEY_REQUIRED",
            "Add an OpenRouter key to this connection in Provider Router.",
        )
    })
}
fn openrouter_payload(request: &Value) -> Result<Value> {
    let mut messages = Vec::new();
    let mut route: Option<(String, String)> = None;
    for message in request["messages"].as_array().into_iter().flatten() {
        let mut message = message.clone();
        if let Some(continuation) = message
            .as_object_mut()
            .and_then(|value| value.remove("continuation"))
        {
            let state = &continuation["state"];
            ensure!(state["format"] == "openrouter_chat", invalid_continuation());
            let upstream = state["upstream"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(invalid_continuation)?;
            let resolved_model = state["resolved_model"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or_else(invalid_continuation)?;
            ensure!(route.as_ref().is_none_or(|(provider, model)| provider == upstream && model == resolved_model), invalid_continuation());
            route = Some((upstream.to_owned(), resolved_model.to_owned()));
            let native = &state["message"];
            let raw_calls = native["tool_calls"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let calls = message["tool_calls"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            ensure!(
                native["role"] == "assistant"
                    && text_content(&native["content"]) == text_content(&message["content"])
                    && raw_calls.len() == calls.len()
                    && raw_calls.iter().zip(calls).all(|(raw, call)| same_call(
                        call,
                        &raw["id"],
                        &raw["function"]["name"],
                        &raw["function"]["arguments"],
                    )),
                invalid_continuation()
            );
            message = native.clone();
        }
        messages.push(message);
    }
    let mut payload = json!({
        "model": request["model"],
        "messages": messages,
        "stream": false,
        "provider": {"require_parameters": true},
    });
    if let Some((upstream, model)) = route {
        payload["model"] = json!(model);
        payload["provider"]["only"] = json!([upstream]);
        payload["provider"]["allow_fallbacks"] = json!(false);
    }
    if let Some(id) = request.get("session_id") {
        payload["session_id"] = id.clone();
    }
    if payload["model"]
        .as_str()
        .is_some_and(|model| model.starts_with("anthropic/"))
    {
        payload["cache_control"] = json!({"type": "ephemeral"});
    }
    if request["tools"]
        .as_array()
        .is_some_and(|tools| !tools.is_empty())
    {
        payload["tools"] = request["tools"].clone();
    }
    if let Some(effort) = request.get("reasoning_effort") {
        payload["reasoning"] = json!({"effort": effort});
    }
    if let Some(maximum) = request.get("max_output_tokens") {
        payload["max_tokens"] = maximum.clone();
    }
    for key in ["response_format", "tool_choice", "parallel_tool_calls"] {
        if let Some(value) = request.get(key) {
            payload[key] = value.clone();
        }
    }
    Ok(payload)
}
fn invalid_continuation() -> anyhow::Error {
    failure(
        400,
        "INVALID_HISTORY",
        "The assistant continuation is incompatible with this request.",
    )
}
struct Completion {
    content: String,
    calls: Vec<Value>,
    input_tokens: u64,
    output_tokens: u64,
    state: Option<Value>,
    cache_read: Option<u64>,
    cache_write: Option<u64>,
}
#[derive(Deserialize)]
struct OpenRouterCompletion {
    choices: Vec<OpenRouterChoice>,
    usage: OpenRouterUsage,
    provider: Option<String>,
    model: Option<String>,
    openrouter_metadata: Option<Value>,
}
impl OpenRouterCompletion {
    fn selected_provider(&self) -> Option<&str> {
        self.provider.as_deref().or_else(|| {
            self.openrouter_metadata.as_ref()?["endpoints"]["available"]
                .as_array()?
                .iter()
                .find(|endpoint| endpoint["selected"] == true)?["provider"]
                .as_str()
        })
    }
}
async fn openrouter_endpoint_tag(
    client: &Client,
    key: &str,
    model: &str,
    provider: &str,
) -> Option<String> {
    // Completion metadata names the provider. The routing allowlist needs its endpoint tag.
    let (author, name) = model.split_once('/')?;
    let mut url = reqwest::Url::parse(OPENROUTER_URL).ok()?;
    url.path_segments_mut()
        .ok()?
        .extend(["models", author, name, "endpoints"]);
    let response = send(
        client
            .get(url)
            .bearer_auth(key)
            .timeout(Duration::from_secs(3)),
    )
    .await
    .ok()?;
    let catalog: Value = bounded_json(response).await.ok()?;
    endpoint_tag(&catalog, provider).map(str::to_owned)
}
fn endpoint_tag<'a>(catalog: &'a Value, provider: &str) -> Option<&'a str> {
    let mut tags = catalog["data"]["endpoints"]
        .as_array()?
        .iter()
        .filter(|entry| entry["provider_name"] == provider || entry["tag"] == provider)
        .filter_map(|entry| entry["tag"].as_str());
    let tag = tags.next()?;
    tags.all(|other| other == tag).then_some(tag)
}
#[derive(Deserialize)]
struct OpenRouterChoice {
    finish_reason: Option<String>,
    message: OpenRouterMessage,
}
#[derive(Deserialize)]
struct OpenRouterMessage {
    content: Option<String>,
    tool_calls: Option<Vec<Value>>,
    reasoning: Option<Value>,
    reasoning_details: Option<Value>,
}
#[derive(Deserialize)]
struct OpenRouterUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
    prompt_tokens_details: Option<Value>,
}
fn openrouter_output(response: &OpenRouterCompletion) -> Result<Completion> {
    let choice = response.choices.first().ok_or_else(invalid_response)?;
    ensure!(
        matches!(choice.finish_reason.as_deref(), Some("stop" | "tool_calls")),
        incomplete()
    );
    let mut native = json!({"role": "assistant", "content": choice.message.content});
    if let Some(calls) = &choice.message.tool_calls {
        native["tool_calls"] = json!(calls);
    }
    if let Some(value) = &choice.message.reasoning_details {
        native["reasoning_details"] = value.clone();
    } else if let Some(value) = &choice.message.reasoning {
        native["reasoning"] = value.clone();
    }
    let state = response.model.as_ref().filter(|_| {
        !native["reasoning_details"].is_null() || !native["reasoning"].is_null()
    }).map(|model| json!({"format": "openrouter_chat", "message": native, "resolved_model": model}));
    let usage_details = response.usage.prompt_tokens_details.as_ref();
    Ok(Completion {
        content: choice.message.content.clone().unwrap_or_default(),
        calls: choice.message.tool_calls.clone().unwrap_or_default(),
        input_tokens: response.usage.prompt_tokens,
        output_tokens: response.usage.completion_tokens,
        state,
        cache_read: usage_details.and_then(|details| details["cached_tokens"].as_u64()),
        cache_write: usage_details.and_then(|details| details["cache_write_tokens"].as_u64()),
    })
}
fn codex_payload(request: &Value) -> Result<Value> {
    let mut input = Vec::new();
    for message in request["messages"].as_array().into_iter().flatten() {
        let role = message["role"].as_str().unwrap_or_default();
        if let Some(continuation) = message.get("continuation") {
            let state = &continuation["state"];
            ensure!(state["format"] == "codex_responses", invalid_continuation());
            let items = state["items"].as_array().ok_or_else(invalid_continuation)?;
            let mut calls = message["tool_calls"].as_array().into_iter().flatten();
            for item in items {
                match item["type"].as_str() {
                    Some("reasoning") if item["encrypted_content"].as_str().is_some() => {}
                    Some("reasoning") => continue,
                    Some("message") if item["role"] == "assistant" => {}
                    Some("function_call")
                        if calls.next().is_some_and(|call| {
                            same_call(call, &item["call_id"], &item["name"], &item["arguments"])
                        }) => {}
                    _ => bail!(invalid_continuation()),
                }
                input.push(item.clone());
            }
            ensure!(calls.next().is_none(), invalid_continuation());
            continue;
        }
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
        }
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
            }
        };
        if !content.is_empty() {
            let role = if role == "system" { "developer" } else { role };
            input.push(json!({"role": role, "content": content}));
        }
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            input.push(json!({
                "type": "function_call", "call_id": call["id"],
                "name": call["function"]["name"], "arguments": call["function"]["arguments"],
            }));
        }
    }
    let tools: Vec<Value> = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|tool| {
            let mut function = tool["function"].clone();
            function["type"] = json!("function");
            function
        })
        .collect();
    let mut payload = json!({
        "model": request["model"], "instructions": "", "input": input,
        "store": false, "stream": true, "tools": tools,
        "include": ["reasoning.encrypted_content"],
    });
    if let Some(id) = request.get("session_id") {
        payload["prompt_cache_key"] = id.clone();
    }
    if let Some(effort) = request.get("reasoning_effort") {
        payload["reasoning"] = json!({"effort": effort, "summary": "auto"});
    }
    if let Some(choice) = request.get("tool_choice") {
        payload["tool_choice"] = if choice.is_object() {
            json!({"type": "function", "name": choice["function"]["name"]})
        } else {
            choice.clone()
        };
    }
    if let Some(parallel) = request.get("parallel_tool_calls") {
        payload["parallel_tool_calls"] = parallel.clone();
    }
    // Codex controls the subscription output budget. The API validates the completed usage.
    if let Some(format) = request["response_format"]["json_schema"].as_object() {
        let mut format = format.clone();
        format.insert("type".into(), json!("json_schema"));
        payload["text"] = json!({"format": format});
    }
    Ok(payload)
}
fn same_call(call: &Value, id: &Value, name: &Value, arguments: &Value) -> bool {
    let decode = |value: &Value| {
        value
            .as_str()
            .and_then(|args| serde_json::from_str::<Value>(args).ok())
    };
    call["id"] == *id
        && call["function"]["name"] == *name
        && decode(arguments).is_some_and(|raw| decode(&call["function"]["arguments"]) == Some(raw))
}
fn text_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}
async fn codex_request(
    client: &Client,
    connection: &Connection,
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
        }
        // Codex subscription cache affinity uses the Responses session header.
        if let Some(session_id) = payload.and_then(|body| body["prompt_cache_key"].as_str()) {
            request = request.header("session-id", session_id);
        }
        let response = request.send().await.map_err(|_| unavailable())?;
        if response.status().as_u16() == 401 && !retry {
            rejected = Some(credentials.access_token);
            continue;
        }
        return check_status(response);
    }
    unreachable!("the final Codex attempt returns a response or error")
}
async fn send(request: RequestBuilder) -> Result<Response> {
    check_status(request.send().await.map_err(|_| unavailable())?)
}
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
    match error["code"]
        .as_u64()
        .and_then(|code| u16::try_from(code).ok())
    {
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
        ),
        429 => failure(
            429,
            "PROVIDER_RATE_LIMIT",
            "The provider is currently rate limited.",
        ),
        401 => failure(
            502,
            "PROVIDER_AUTH_REQUIRED",
            "The provider rejected the credential. Check the connection in Provider Router.",
        ),
        _ => unavailable(),
    }
}
fn unavailable() -> anyhow::Error {
    failure(
        502,
        "PROVIDER_UNAVAILABLE",
        "The provider request failed. Check the connection in Provider Router.",
    )
}
async fn bounded_json<T: serde::de::DeserializeOwned>(mut response: Response) -> Result<T> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        ensure!(
            bytes.len() + chunk.len() <= MAX_PROVIDER_BYTES,
            output_limit()
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| invalid_response())
}
fn output_limit() -> anyhow::Error {
    failure(
        502,
        "PROVIDER_OUTPUT_LIMIT",
        "The provider response exceeded the supported size.",
    )
}
/// Streamed upstream failures keep the meaning of the equivalent HTTP status.
fn stream_failure(error: &Value) -> anyhow::Error {
    match error["code"].as_str() {
        Some("server_error") => unavailable(),
        Some("rate_limit_exceeded") => failure(
            429,
            "PROVIDER_RATE_LIMIT",
            "The provider is currently rate limited.",
        ),
        _ => incomplete(),
    }
}
fn incomplete() -> anyhow::Error {
    failure(
        502,
        "INCOMPLETE_COMPLETION",
        "The provider did not finish the response.",
    )
}
async fn completed_response(mut response: Response) -> Result<Value> {
    let mut decoder = CodexSse::default();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if let Some(completion) = decoder.feed(&chunk)? {
            return Ok(completion);
        }
    }
    bail!(incomplete())
}
/// Framing stays byte-oriented until all lines of the SSE event have arrived.
#[derive(Default)]
struct CodexSse {
    line: Vec<u8>,
    data: Vec<u8>,
    received: usize,
    completed_items: BTreeMap<u64, Value>,
}
impl CodexSse {
    fn feed(&mut self, chunk: &[u8]) -> Result<Option<Value>> {
        self.received += chunk.len();
        ensure!(self.received <= MAX_PROVIDER_BYTES, output_limit());
        for line in chunk.split_inclusive(|byte| *byte == b'\n') {
            self.line.extend_from_slice(line);
            if !line.ends_with(b"\n") {
                continue;
            }
            let line = self.line.strip_suffix(b"\n").unwrap_or_default();
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() {
                if !self.data.is_empty() && self.data != b"[DONE]" {
                    let event: Value =
                        serde_json::from_slice(&self.data).map_err(|_| invalid_response())?;
                    match event["type"].as_str() {
                        Some("response.completed") if event["response"].is_object() => {
                            let mut response = event["response"].clone();
                            let terminal_count = response["output"].as_array().map_or(0, Vec::len);
                            if !self.completed_items.is_empty()
                                && self.completed_items.len() >= terminal_count
                                && self
                                    .completed_items
                                    .keys()
                                    .copied()
                                    .eq(0..self.completed_items.len() as u64)
                            {
                                let mut items = std::mem::take(&mut self.completed_items);
                                if let Some(terminal_items) = response["output"].as_array() {
                                    for terminal_item in terminal_items {
                                        if let Some(id) = terminal_item["id"].as_str()
                                            && let Some(item) =
                                                items.values_mut().find(|item| item["id"] == id)
                                        {
                                            item.as_object_mut()
                                                .expect("SSE output items are objects")
                                                .extend(
                                                    terminal_item
                                                        .as_object()
                                                        .expect("an item with an ID is an object")
                                                        .clone(),
                                                );
                                        }
                                    }
                                }
                                response["output"] = Value::Array(items.into_values().collect());
                            }
                            return Ok(Some(response));
                        }
                        Some("response.output_item.done") if event["item"].is_object() => {
                            let index = event["output_index"]
                                .as_u64()
                                .unwrap_or(self.completed_items.len() as u64);
                            self.completed_items.insert(index, event["item"].clone());
                        }
                        Some("response.failed") => {
                            bail!(stream_failure(&event["response"]["error"]))
                        }
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
            }
            self.line.clear();
        }
        Ok(None)
    }
}
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
            }
            Some("message") => {
                for part in item["content"].as_array().ok_or_else(invalid_response)? {
                    match part["type"].as_str() {
                        Some("output_text") => {
                            content.push_str(part["text"].as_str().ok_or_else(invalid_response)?);
                        }
                        Some("refusal") => bail!(failure(
                            422,
                            "PROVIDER_REFUSAL",
                            "The provider declined this request."
                        )),
                        _ => {}
                    }
                }
            }
            Some("reasoning") => {}
            _ => bail!(failure(
                502,
                "UNSUPPORTED_PROVIDER_OUTPUT",
                "The subscription returned an unsupported output item."
            )),
        }
    }
    Ok(Completion {
        content,
        calls,
        input_tokens: response["usage"]["input_tokens"]
            .as_u64()
            .ok_or_else(invalid_response)?,
        output_tokens: response["usage"]["output_tokens"]
            .as_u64()
            .ok_or_else(invalid_response)?,
        state: Some(json!({"format": "codex_responses", "items": response["output"]})),
        cache_read: response["usage"]["input_tokens_details"]["cached_tokens"].as_u64(),
        cache_write: response["usage"]["input_tokens_details"]["cache_write_tokens"].as_u64(),
    })
}
#[derive(Deserialize)]
struct CodexCatalog {
    models: Vec<CodexModel>,
}
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
}
fn codex_model(entry: CodexModel) -> Option<Model> {
    let context = entry.context_window.filter(|maximum| *maximum > 0)?;
    if entry.slug.is_empty() || hidden_codex_model(&entry.slug) {
        return None;
    }
    let efforts: Vec<String> = entry
        .supported_reasoning_levels
        .iter()
        .filter_map(|value| {
            value
                .as_str()
                .or_else(|| value["effort"].as_str())
                .map(str::to_owned)
        })
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
    })
}
#[derive(Deserialize)]
struct OpenRouterCatalog {
    data: Vec<OpenRouterModel>,
}
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
}
#[derive(Deserialize)]
struct OpenRouterMaximum {
    max_completion_tokens: Option<u64>,
}
#[derive(Default, Deserialize)]
struct OpenRouterArchitecture {
    #[serde(default)]
    input_modalities: Vec<String>,
}
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
    }
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
    })
}
fn modalities(input: &[String]) -> Vec<String> {
    let mut output = vec!["text".into()];
    if input.iter().any(|value| value == "image") {
        output.push("image".into());
    }
    output
}
pub struct Catalog(reqwest::Client);
impl Catalog {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self(client))
    }
    pub async fn models(
        &self,
        store: &impl SettingsRepository,
        connection: &Connection,
    ) -> Result<Vec<Model>> {
        management::catalog(store, connection, async |connection, key| {
            models(&self.0, connection, key).await
        })
        .await
    }
}
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
        let ids: Vec<String> = catalog
            .models
            .into_iter()
            .filter_map(codex_model)
            .map(|model| model.id)
            .collect();
        assert_eq!(ids, ["gpt-6-sol"]);
    }
    #[test]
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
    }
    #[test]
    fn codex_stream_retains_reasoning_in_completed_output_item_events() {
        let reasoning = json!({"type":"reasoning","id":"rs_test","encrypted_content":"opaque"});
        let terminal_reasoning = json!({"type":"reasoning","id":"rs_test","summary":[]});
        let call = json!({"type":"function_call","id":"fc_test","call_id":"call_test","name":"lookup","arguments":"{}"});
        let events = [
            json!({"type":"response.output_item.done", "item":reasoning}),
            json!({"type":"response.output_item.done", "item":call}),
            json!({"type":"response.completed", "response":{"status":"completed", "output":[terminal_reasoning,call]}}),
        ];
        let mut decoder = CodexSse::default();
        let outcome = events
            .iter()
            .find_map(|event| {
                decoder
                    .feed(format!("data: {event}\n\n").as_bytes())
                    .unwrap()
            })
            .unwrap();
        assert_eq!(outcome["output"][0]["encrypted_content"], "opaque");
        assert_eq!(outcome["output"][0]["summary"], json!([]));
        assert_eq!(outcome["output"][1], call);
    }
    #[test]
    fn openrouter_body_errors_keep_upstream_status_meaning() {
        let cases = [
            (
                json!({"code": 502, "message": "Provider disconnected"}),
                502,
                "PROVIDER_UNAVAILABLE",
            ),
            (
                json!({"code": 402, "message": "Insufficient credits"}),
                502,
                "PROVIDER_UNAVAILABLE",
            ),
            (
                json!({"code": 429, "message": "Rate limited"}),
                429,
                "PROVIDER_RATE_LIMIT",
            ),
            (
                json!({"code": 400, "message": "Bad request"}),
                400,
                "PROVIDER_REQUEST_REJECTED",
            ),
            (json!({"message": "Unknown"}), 502, "INCOMPLETE_COMPLETION"),
        ];
        for (error, status, code) in cases {
            let top_level = upstream_body_failure(&error)
                .downcast::<ProviderError>()
                .unwrap();
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
    #[test]
    fn codex_stream_failures_keep_upstream_availability_meaning() {
        let cases: [(&[u8], u16, &str); 5] = [
            (b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\"}}}\n\n", 502, "PROVIDER_UNAVAILABLE"),
            (b"data: {\"type\":\"error\",\"code\":\"server_error\"}\n\n", 502, "PROVIDER_UNAVAILABLE"),
            (b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"rate_limit_exceeded\"}}}\n\n", 429, "PROVIDER_RATE_LIMIT"),
            (b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"invalid_prompt\"}}}\n\n", 502, "INCOMPLETE_COMPLETION"),
            (b"data: {\"type\":\"response.incomplete\"}\n\n", 502, "INCOMPLETE_COMPLETION"),
        ];
        for (event, status, code) in cases {
            let error = CodexSse::default()
                .feed(event)
                .unwrap_err()
                .downcast::<ProviderError>()
                .unwrap();
            assert_eq!((error.status, error.code), (status, code));
        }
    }
    #[test]
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
    }
    #[test]
    fn codex_replays_the_original_ordered_output_before_tool_results() {
        let reasoning =
            json!({"type":"reasoning", "id":"rs_test", "encrypted_content":"opaque", "summary":[]});
        let call = json!({"type":"function_call", "id":"fc_test", "call_id":"call_test", "name":"lookup", "arguments":"{ }"});
        let completion = codex_output(json!({
            "status":"completed", "output":[reasoning, call],
            "usage":{"input_tokens":1200,"output_tokens":20,"input_tokens_details":{"cached_tokens":1024}},
        })).unwrap();
        let payload = codex_payload(&json!({"model":"test", "messages":[
            {"role":"user","content":"Find the value"},
            {"role":"assistant","content":"","tool_calls":[{"type":"function","id":"call_test",
                "function":{"name":"lookup","arguments":"{}"}}],"continuation":{"state":completion.state}},
            {"role":"tool","tool_call_id":"call_test","content":"value"},
        ]})).unwrap();
        assert_eq!(payload["input"][1], reasoning);
        assert_eq!(payload["input"][2], call);
        assert_eq!(payload["input"][3]["call_id"], "call_test");
        assert_eq!(payload["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(completion.cache_read, Some(1024));
    }
    #[test]
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
        let payload = openrouter_payload(&request).unwrap();
        assert_eq!(payload["tools"][0], tool);
        assert_eq!(payload["tool_choice"], "required");
        assert_eq!(payload["reasoning"]["effort"], "low");
        assert_eq!(payload["max_tokens"], 120);
        let tool_call = json!({"type":"function","id":"call_test","function":{"name":"lookup","arguments":"{}"}});
        let reasoning = json!([{"type":"reasoning.encrypted", "id":"rs_test", "data":"opaque"}]);
        let mut completion = json!({
            "model":"test/resolved",
            "openrouter_metadata":{"endpoints":{"available":[{"selected":true,"provider":"OpenAI"}]}},
            "choices":[{"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[tool_call],"reasoning_details":reasoning}}],
            "usage":{"prompt_tokens":14,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":7,"cache_write_tokens":4}},
        });
        let upstream: OpenRouterCompletion = serde_json::from_value(completion.clone()).unwrap();
        let endpoints = json!({"data":{"endpoints":[{"provider_name":"OpenAI","tag":"openai"}]}});
        let tag = endpoint_tag(&endpoints, upstream.selected_provider().unwrap()).unwrap();
        let mut accepted = openrouter_output(&upstream).unwrap();
        accepted.state.as_mut().unwrap()["upstream"] = json!(tag);
        assert_eq!(accepted.calls, [tool_call.clone()]);
        assert_eq!((accepted.input_tokens, accepted.output_tokens), (14, 3));
        assert_eq!(
            (accepted.cache_read, accepted.cache_write),
            (Some(7), Some(4))
        );
        let continuation = openrouter_payload(&json!({"model":"test/model", "session_id":"test-session", "messages":[
            {"role":"assistant","content":"","tool_calls":[tool_call],"continuation":{"state":accepted.state}},
            {"role":"tool","tool_call_id":"call_test","content":"result"},
        ]})).unwrap();
        assert_eq!(continuation["messages"][0]["reasoning_details"], reasoning);
        assert_eq!(continuation["provider"]["only"], json!(["openai"]));
        assert_eq!(continuation["model"], "test/resolved");
        assert_eq!(continuation["session_id"], "test-session");
        completion["choices"][0]["finish_reason"] = "length".into();
        assert!(openrouter_output(&serde_json::from_value(completion).unwrap()).is_err());
    }
    #[test]
    fn openrouter_tool_calls_reject_unknown_fields_at_the_completion_boundary() {
        let fixtures: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/router-wire-parity.json"))
                .unwrap();
        let upstream: OpenRouterCompletion = serde_json::from_value(json!({
            "choices": [{"finish_reason": "tool_calls", "message": {"tool_calls": [fixtures["openRouterToolCall"]]}}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2}
        }))
        .unwrap();
        let output = openrouter_output(&upstream).unwrap();
        assert!(
            serde_json::from_value::<Vec<crate::application::contracts::ToolCall>>(json!(
                output.calls
            ))
            .is_err()
        );
    }
    #[test]
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
        ];
        for (upstream, status, code) in cases {
            let response = axum::http::Response::builder()
                .status(upstream)
                .body("upstream-private-detail")
                .unwrap();
            let error = check_status(response.into())
                .unwrap_err()
                .downcast::<ProviderError>()
                .unwrap();
            assert_eq!(
                (error.status, error.code),
                (status, code),
                "HTTP {upstream}"
            );
            if status == 400 {
                assert_eq!(
                    error.message,
                    "The provider rejected the request as invalid."
                );
            }
        }
    }
    #[tokio::test]
    async fn upstream_transport_failure_remains_unavailable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let disconnect = tokio::spawn(async move { drop(listener.accept().await.unwrap()) });
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let error = send(client.post(format!("http://{address}")))
            .await
            .unwrap_err()
            .downcast::<ProviderError>()
            .unwrap();
        assert_eq!((error.status, error.code), (502, "PROVIDER_UNAVAILABLE"));
        disconnect.await.unwrap();
    }
}
