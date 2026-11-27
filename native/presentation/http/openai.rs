//! Translate the supported OpenAI request shapes and produce standard SDK envelopes.

use crate::application::contracts::{
    Attachment, Choice, Completion, Content, Continuation, ContinuationState, Format, Function,
    Invocation, Message, Part, Request, Role, Schema, Tool, ToolCall, is_compaction_item,
    parse_choice,
};
use crate::application::failure::{Failure, Outcome};
use crate::domain::Connection;
use axum::{
    Json,
    http::header,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Clone, Copy)]
pub(super) enum Surface {
    Chat,
    Responses,
}

pub(super) struct OpenAi {
    pub request: Request,
    pub model: String,
    stream: bool,
    usage: bool,
    surface: Surface,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chat {
    model: String,
    messages: Vec<Message>,
    #[serde(default)]
    tools: Vec<Tool>,
    response_format: Option<Format>,
    reasoning_effort: Option<String>,
    max_tokens: Option<u64>,
    max_completion_tokens: Option<u64>,
    #[serde(default)]
    stream: bool,
    stream_options: Option<StreamOptions>,
    tool_choice: Option<Value>,
    parallel_tool_calls: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StreamOptions {
    #[serde(default)]
    include_usage: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Responses {
    model: String,
    input: Input,
    instructions: Option<String>,
    #[serde(default)]
    tools: Vec<FlatTool>,
    text: Option<Text>,
    reasoning: Option<Reasoning>,
    max_output_tokens: Option<u64>,
    #[serde(default)]
    stream: bool,
    store: Option<bool>,
    tool_choice: Option<Value>,
    parallel_tool_calls: Option<bool>,
    provider_router_messages: Option<Vec<Message>>,
    session_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FlatTool {
    Function(Function),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    format: FlatFormat,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FlatFormat {
    JsonSchema(Schema),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reasoning {
    effort: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Input {
    Text(String),
    History(Vec<InputItem>),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum InputItem {
    Function(HistoryCall),
    Output(HistoryOutput),
    Message(HistoryMessage),
    Native(Value),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum HistoryCall {
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
        #[serde(rename = "id")]
        _id: Option<String>,
        #[serde(rename = "status")]
        _status: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum HistoryOutput {
    FunctionCallOutput {
        call_id: String,
        output: String,
        #[serde(rename = "id")]
        _id: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryMessage {
    role: Role,
    #[serde(rename = "type")]
    kind: Option<String>,
    content: HistoryContent,
    #[serde(rename = "id")]
    _id: Option<String>,
    #[serde(rename = "status")]
    _status: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum HistoryContent {
    Text(String),
    Parts(Vec<HistoryPart>),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum HistoryPart {
    InputText {
        text: String,
    },
    OutputText {
        text: String,
    },
    InputFile {
        filename: Option<String>,
        file_data: Option<String>,
        file_id: Option<String>,
    },
}

impl HistoryPart {
    fn into_part(self) -> Outcome<Part> {
        // Responses parts accept extension metadata while document sources are exclusive.
        match self {
            Self::InputText { text } | Self::OutputText { text } => Ok(Part::Text { text }),
            Self::InputFile {
                filename: Some(filename),
                file_data: Some(file_data),
                file_id: None,
            } => Ok(Part::File {
                file: Attachment::Inline {
                    filename,
                    file_data,
                },
            }),
            Self::InputFile {
                filename: None,
                file_data: None,
                file_id: Some(file_id),
            } => Ok(Part::File {
                file: Attachment::Reference { file_id },
            }),
            Self::InputFile { .. } => Err(Failure::invalid()),
        }
    }
}

pub(super) fn decode(
    surface: Surface,
    value: Value,
    connections: &[Connection],
    request_id: Uuid,
) -> Outcome<OpenAi> {
    match surface {
        Surface::Chat => {
            let input: Chat = serde_json::from_value(value).map_err(|_| Failure::invalid())?;
            if input.max_tokens.is_some() && input.max_completion_tokens.is_some() {
                return Err(Failure::invalid());
            }
            let (connection_id, model) = select(&input.model, connections)?;
            let tools = active_tools(input.tools, &input.tool_choice)?;
            Ok(OpenAi {
                model: format!("{connection_id}/{model}"),
                request: Request {
                    request_id,
                    connection_id,
                    model,
                    messages: input.messages,
                    tools,
                    reasoning_effort: input.reasoning_effort,
                    response_format: input.response_format,
                    max_output_tokens: input.max_completion_tokens.or(input.max_tokens),
                    tool_choice: input.tool_choice,
                    parallel_tool_calls: input.parallel_tool_calls,
                    session_id: None,
                    compaction: None,
                },
                stream: input.stream,
                usage: input
                    .stream_options
                    .is_some_and(|value| value.include_usage),
                surface,
            })
        }
        Surface::Responses => {
            let mut input: Responses =
                serde_json::from_value(value).map_err(|_| Failure::invalid())?;
            if input.store == Some(true) {
                return Err(Failure::invalid());
            }
            let (connection_id, model) = select(&input.model, connections)?;
            let compaction = remove_compaction_trigger(&mut input.input)?;
            let mut messages = match input.provider_router_messages {
                Some(messages) => messages,
                None => history(input.input, connection_id, &model)?,
            };
            if let Some(instructions) = input.instructions {
                messages.insert(0, Message::text(Role::System, instructions));
            }
            let tool_choice = input.tool_choice.map(|choice| {
                if choice["type"] == "function" {
                    json!({"type":"function", "function":{"name":choice["name"]}})
                } else {
                    choice
                }
            });
            let tools = active_tools(
                input
                    .tools
                    .into_iter()
                    .map(|FlatTool::Function(function)| Tool::Function { function })
                    .collect(),
                &tool_choice,
            )?;
            let response_format = input.text.map(
                |Text {
                     format: FlatFormat::JsonSchema(json_schema),
                 }| Format::JsonSchema { json_schema },
            );
            Ok(OpenAi {
                model: format!("{connection_id}/{model}"),
                request: Request {
                    request_id,
                    connection_id,
                    model,
                    messages,
                    tools,
                    reasoning_effort: input.reasoning.and_then(|value| value.effort),
                    response_format,
                    max_output_tokens: input.max_output_tokens,
                    tool_choice,
                    parallel_tool_calls: input.parallel_tool_calls,
                    session_id: input.session_id,
                    compaction: compaction.then_some(true),
                },
                stream: input.stream,
                usage: true,
                surface,
            })
        }
    }
}

fn select(name: &str, connections: &[Connection]) -> Outcome<(Uuid, String)> {
    let mut choices = connections
        .iter()
        .filter(|connection| connection.exposed)
        .flat_map(|connection| {
            connection
                .models
                .iter()
                .filter(move |model| {
                    name == model.id || name == format!("{}/{}", connection.id, model.id)
                })
                .map(|model| (connection.id, model.id.clone()))
        });
    let choice = choices.next().ok_or_else(|| {
        Failure::missing(
            "MODEL_UNAVAILABLE",
            "This model is not exposed by Provider Router.",
        )
    })?;
    if choices.next().is_some() {
        return Err(Failure::bad(
            "MODEL_AMBIGUOUS",
            "Multiple connections expose this model. Use the qualified model identifier from /v1/models.",
        ));
    }
    Ok(choice)
}

fn active_tools(mut tools: Vec<Tool>, choice: &Option<Value>) -> Outcome<Vec<Tool>> {
    if choice
        .as_ref()
        .map(parse_choice)
        .transpose()?
        .is_some_and(|choice| matches!(choice, Choice::None))
    {
        tools.clear();
    }
    Ok(tools)
}

fn remove_compaction_trigger(input: &mut Input) -> Outcome<bool> {
    let Input::History(items) = input else {
        return Ok(false);
    };
    let mut compact = false;
    for (index, item) in items.iter().enumerate() {
        let InputItem::Native(item) = item else {
            continue;
        };
        if item["type"] == "compaction_trigger"
            && index + 1 == items.len()
            && item.as_object().is_some_and(|fields| fields.len() == 1)
        {
            compact = true;
        } else if !is_compaction_item(item) {
            return Err(Failure::invalid());
        }
    }
    if compact {
        items.pop();
    }
    Ok(compact)
}

fn history(input: Input, connection_id: Uuid, model: &str) -> Outcome<Vec<Message>> {
    match input {
        Input::Text(text) => Ok(vec![Message::text(Role::User, text)]),
        Input::History(items) => items
            .into_iter()
            .map(|item| match item {
                InputItem::Function(HistoryCall::FunctionCall {
                    call_id,
                    name,
                    arguments,
                    ..
                }) => {
                    let mut message = Message::text(Role::Assistant, String::new());
                    message.tool_calls = Some(vec![ToolCall::Function {
                        id: call_id,
                        function: Invocation { name, arguments },
                    }]);
                    Ok(message)
                }
                InputItem::Output(HistoryOutput::FunctionCallOutput {
                    call_id, output, ..
                }) => {
                    let mut message = Message::text(Role::Tool, output);
                    message.tool_call_id = Some(call_id);
                    Ok(message)
                }
                InputItem::Native(item) if is_compaction_item(&item) => {
                    let mut message = Message::text(Role::Assistant, String::new());
                    message.continuation = Some(Continuation {
                        connection_id,
                        model: model.to_owned(),
                        state: ContinuationState::CodexResponses { items: vec![item] },
                    });
                    Ok(message)
                }
                InputItem::Native(_) => Err(Failure::invalid()),
                InputItem::Message(input) => {
                    if matches!(input.role, Role::Tool)
                        || input.kind.is_some_and(|kind| kind != "message")
                    {
                        return Err(Failure::invalid());
                    }
                    let content = match input.content {
                        HistoryContent::Text(text) => Content::Text(text),
                        HistoryContent::Parts(parts) => Content::Parts(
                            parts
                                .into_iter()
                                .map(HistoryPart::into_part)
                                .collect::<Outcome<_>>()?,
                        ),
                    };
                    Ok(Message {
                        role: input.role,
                        content,
                        name: None,
                        tool_calls: None,
                        tool_call_id: None,
                        continuation: None,
                    })
                }
            })
            .collect(),
    }
}

pub(super) fn response(input: &OpenAi, completion: &Completion) -> Response {
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut provenance = json!({
        "provider": completion.provider, "method": completion.method,
        "connection_id": input.request.connection_id, "model": completion.model,
    });
    if let Some(effort) = &completion.reasoning_effort {
        provenance["reasoning_effort"] = json!(effort);
    }
    if matches!(input.surface, Surface::Responses)
        && !input.stream
        && let Some(continuation) = &completion.continuation
    {
        provenance["continuation"] = json!(continuation);
    }
    let mut cache_usage = serde_json::Map::new();
    if let Some(tokens) = completion.usage.cached_input_tokens {
        cache_usage.insert("cached_tokens".into(), json!(tokens));
    }
    if let Some(tokens) = completion.usage.cache_write_tokens {
        cache_usage.insert("cache_write_tokens".into(), json!(tokens));
    }
    let total = completion
        .usage
        .input_tokens
        .saturating_add(completion.usage.output_tokens);
    match input.surface {
        Surface::Chat => {
            let id = format!("chatcmpl-{}", completion.request_id);
            let usage = json!({"prompt_tokens":completion.usage.input_tokens, "completion_tokens":completion.usage.output_tokens,
                "total_tokens":total, "prompt_tokens_details":cache_usage});
            let finish = if completion.tool_calls.is_empty() {
                "stop"
            } else {
                "tool_calls"
            };
            if !input.stream {
                let mut message = json!({"role":"assistant", "content": (!completion.content.is_empty()).then_some(&completion.content)});
                if !completion.tool_calls.is_empty() {
                    message["tool_calls"] = json!(completion.tool_calls);
                }
                return Json(json!({"id":id, "object":"chat.completion", "created":created, "model":input.model,
                    "choices":[{"index":0, "message":message, "finish_reason":finish,"logprobs":null}], "usage":usage, "provider_router":provenance})).into_response();
            }
            let chunk = |delta, finish_reason: Option<&str>| {
                json!({"id":id, "object":"chat.completion.chunk", "created":created,
                "model":input.model, "choices":[{"index":0, "delta":delta, "finish_reason":finish_reason, "logprobs":null}], "provider_router":provenance})
            };
            let mut body = String::new();
            sse(
                &mut body,
                None,
                &chunk(json!({"role":"assistant","content":""}), None),
            );
            if !completion.content.is_empty() {
                sse(
                    &mut body,
                    None,
                    &chunk(json!({"content":completion.content}), None),
                );
            }
            if !completion.tool_calls.is_empty() {
                let calls: Vec<_> = completion
                    .tool_calls
                    .iter()
                    .enumerate()
                    .map(|(index, call)| {
                        let mut value = json!(call);
                        value["index"] = json!(index);
                        value
                    })
                    .collect();
                sse(&mut body, None, &chunk(json!({"tool_calls":calls}), None));
            }
            sse(&mut body, None, &chunk(json!({}), Some(finish)));
            if input.usage {
                sse(
                    &mut body,
                    None,
                    &json!({"id":id, "object":"chat.completion.chunk", "created":created, "model":input.model, "choices":[], "usage":usage}),
                );
            }
            body.push_str("data: [DONE]\n\n");
            stream(body)
        }
        Surface::Responses => {
            let mut output: Vec<Value> = completion
                .continuation
                .as_ref()
                .and_then(|continuation| continuation.state.compaction_item())
                .into_iter()
                .cloned()
                .collect();
            if !completion.content.is_empty() {
                output.push(json!({"type":"message", "id":format!("msg_{}", Uuid::now_v7()), "role":"assistant", "status":"completed",
                    "content":[{"type":"output_text","text":completion.content,"annotations":[],"logprobs":[]}]}));
            }
            output.extend(completion.tool_calls.iter().map(|call| {
                let (id, function) = call.parts();
                json!({"type":"function_call","id":format!("fc_{}", Uuid::now_v7()),"call_id":id,"name":function.name,
                    "arguments":function.arguments,"status":"completed"})
            }));
            let mut choice = input.request.tool_choice.clone().unwrap_or(json!("auto"));
            if choice.is_object() {
                choice = json!({"type":"function","name":choice["function"]["name"]});
            }
            let tools: Vec<_> = input
                .request
                .tools
                .iter()
                .map(|tool| {
                    let mut value = json!(tool.function());
                    value["type"] = json!("function");
                    value
                })
                .collect();
            let mut object = json!({"id":format!("resp_{}", completion.request_id), "object":"response","created_at":created,"status":"completed",
                "error":null,"incomplete_details":null,"model":input.model,"output":output,"parallel_tool_calls":input.request.parallel_tool_calls.unwrap_or(true),
                "tool_choice":choice,"tools":tools,"metadata":{},"store":false,"provider_router":provenance,
                "usage":{"input_tokens":completion.usage.input_tokens,"output_tokens":completion.usage.output_tokens,"total_tokens":total}});
            if !cache_usage.is_empty() {
                object["usage"]["input_tokens_details"] = json!(cache_usage);
            }
            if !input.stream {
                return Json(object).into_response();
            }
            stream(response_events(object, &output))
        }
    }
}

fn response_events(object: Value, items: &[Value]) -> String {
    let mut body = String::new();
    let mut sequence = 0;
    let mut emit = |name: &str, mut payload: Value| {
        payload["type"] = json!(name);
        payload["sequence_number"] = json!(sequence);
        sequence += 1;
        sse(&mut body, Some(name), &payload);
    };
    let mut pending = object.clone();
    pending["output"] = json!([]);
    pending["usage"] = Value::Null;
    pending["status"] = json!("in_progress");
    emit("response.created", json!({"response":pending}));
    emit("response.in_progress", json!({"response":pending}));
    for (index, item) in items.iter().enumerate() {
        let mut pending = item.clone();
        pending["status"] = json!("in_progress");
        let id = &item["id"];
        if item["type"] == "message" {
            pending["content"] = json!([]);
            emit(
                "response.output_item.added",
                json!({"output_index":index, "item":pending}),
            );
            let part = &item["content"][0];
            let mut empty = part.clone();
            empty["text"] = json!("");
            emit(
                "response.content_part.added",
                json!({"item_id":id,"output_index":index,"content_index":0,"part":empty}),
            );
            emit(
                "response.output_text.delta",
                json!({"item_id":id,"output_index":index,"content_index":0,"delta":part["text"],"logprobs":[]}),
            );
            emit(
                "response.output_text.done",
                json!({"item_id":id,"output_index":index,"content_index":0,"text":part["text"],"logprobs":[]}),
            );
            emit(
                "response.content_part.done",
                json!({"item_id":id,"output_index":index,"content_index":0,"part":part}),
            );
        } else if item["type"] == "function_call" {
            pending["arguments"] = json!("");
            emit(
                "response.output_item.added",
                json!({"output_index":index, "item":pending}),
            );
            emit(
                "response.function_call_arguments.delta",
                json!({"item_id":id,"output_index":index,"delta":item["arguments"]}),
            );
            emit(
                "response.function_call_arguments.done",
                json!({"item_id":id,"output_index":index,"arguments":item["arguments"],"name":item["name"]}),
            );
        }
        emit(
            "response.output_item.done",
            json!({"output_index":index,"item":item}),
        );
    }
    emit("response.completed", json!({"response":object}));
    body
}

fn sse(body: &mut String, event: Option<&str>, payload: &Value) {
    if let Some(event) = event {
        body.push_str("event: ");
        body.push_str(event);
        body.push('\n');
    }
    body.push_str("data: ");
    body.push_str(&payload.to_string());
    body.push_str("\n\n");
}

fn stream(body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-cache, no-store"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::contracts::Usage;
    use crate::domain::{Method, Model};
    use axum::body::to_bytes;

    fn wire_parity_fixtures() -> Value {
        serde_json::from_str(include_str!(
            "../../../tests/fixtures/router-wire-parity.json"
        ))
        .unwrap()
    }

    fn connection() -> Connection {
        let mut connection = Connection::new(Method::Codex);
        connection.exposed = true;
        connection.models.push(Model {
            id: "test-model".into(),
            name: "Test model".into(),
            reasoning_efforts: vec!["low".into()],
            default_reasoning_effort: Some("low".into()),
            context_window: 128_000,
            max_output_tokens: 4_000,
            tools: true,
            structured_output: true,
            input_modalities: vec!["text".into()],
        });
        connection
    }

    #[test]
    fn completion_checks_native_json_tool_arguments() {
        let connection = connection();
        let request_id = Uuid::now_v7();
        let request = decode(
            Surface::Responses,
            json!({
                "model": "test-model",
                "input": "Synthetic question",
                "tools": [{"type": "function", "name": "lookup", "parameters": {"type": "object"}}]
            }),
            std::slice::from_ref(&connection),
            request_id,
        )
        .unwrap();
        let fixtures = wire_parity_fixtures();
        let mut completion = Completion {
            request_id,
            content: String::new(),
            tool_calls: vec![],
            usage: Usage {
                input_tokens: 5,
                output_tokens: 2,
                cached_input_tokens: None,
                cache_write_tokens: None,
            },
            provider: "codex".into(),
            method: "codex_http".into(),
            model: "test-model".into(),
            reasoning_effort: None,
            continuation: None,
        };
        for fixture in fixtures["toolArguments"].as_array().unwrap() {
            completion.tool_calls = vec![ToolCall::Function {
                id: "call-1".into(),
                function: Invocation {
                    name: "lookup".into(),
                    arguments: fixture["source"].as_str().unwrap().into(),
                },
            }];
            let result = request.request.evaluate(&completion, &connection);
            assert_eq!(result.is_ok(), fixture["accepted"], "{}", fixture["label"]);
            if let Err(failure) = result {
                assert_eq!(
                    (failure.status, failure.code),
                    (502, "INVALID_TOOL_ARGUMENTS")
                );
            }
        }
    }

    #[test]
    fn structured_answers_share_native_json_acceptance_with_tool_arguments() {
        let connection = connection();
        let request_id = Uuid::now_v7();
        let request = decode(
            Surface::Responses,
            json!({
                "model": "test-model",
                "input": "Synthetic question",
                "text": {"format": {"type": "json_schema", "name": "answer", "schema": {"type": "object"}}}
            }),
            std::slice::from_ref(&connection),
            request_id,
        )
        .unwrap();
        for fixture in wire_parity_fixtures()["toolArguments"].as_array().unwrap() {
            let completion = Completion {
                request_id,
                content: fixture["source"].as_str().unwrap().into(),
                tool_calls: vec![],
                usage: Usage {
                    input_tokens: 2,
                    output_tokens: 1,
                    cached_input_tokens: None,
                    cache_write_tokens: None,
                },
                provider: "codex".into(),
                method: "codex_http".into(),
                model: "test-model".into(),
                reasoning_effort: None,
                continuation: None,
            };
            let result = request.request.evaluate(&completion, &connection);
            assert_eq!(result.is_ok(), fixture["accepted"], "{}", fixture["label"]);
            if let Err(failure) = result {
                assert_eq!((failure.status, failure.code), (502, "INVALID_COMPLETION"));
            }
        }
    }

    #[test]
    fn structured_answers_and_tools_share_json_schema_outcomes() {
        let connection = connection();
        let model = &connection.models[0];
        let mut mismatches = Vec::new();
        for fixture in wire_parity_fixtures()["jsonSchemas"].as_array().unwrap() {
            let schema: Value = serde_json::from_str(fixture["schema"].as_str().unwrap()).unwrap();
            let source = fixture["source"].as_str().unwrap();
            for tool in [false, true] {
                let request_id = Uuid::now_v7();
                let payload = if tool {
                    json!({
                        "model": model.id, "input": "Question",
                        "tools": [{"type": "function", "name": "lookup", "parameters": schema}],
                    })
                } else {
                    json!({
                        "model": model.id, "input": "Question",
                        "text": {"format": {"type": "json_schema", "name": "answer", "schema": schema}},
                    })
                };
                let request = decode(
                    Surface::Responses,
                    payload,
                    std::slice::from_ref(&connection),
                    request_id,
                )
                .unwrap();
                let expected = match fixture["code"].as_str() {
                    Some("INVALID_SCHEMA") => Some((400, "INVALID_SCHEMA")),
                    Some("INVALID_COMPLETION") if tool => Some((502, "INVALID_TOOL_ARGUMENTS")),
                    Some("INVALID_COMPLETION") => Some((502, "INVALID_COMPLETION")),
                    None => None,
                    code => panic!("Unexpected fixture code {code:?}"),
                };
                let completion = Completion {
                    request_id,
                    content: if tool { String::new() } else { source.into() },
                    tool_calls: if tool {
                        vec![ToolCall::Function {
                            id: "call-1".into(),
                            function: Invocation {
                                name: "lookup".into(),
                                arguments: source.into(),
                            },
                        }]
                    } else {
                        vec![]
                    },
                    usage: Usage {
                        input_tokens: 2,
                        output_tokens: 1,
                        cached_input_tokens: None,
                        cache_write_tokens: None,
                    },
                    provider: "codex".into(),
                    method: "codex_http".into(),
                    model: model.id.clone(),
                    reasoning_effort: None,
                    continuation: None,
                };
                let outcome = request
                    .request
                    .validate(model)
                    .and_then(|()| request.request.evaluate(&completion, &connection));
                let actual = outcome.err().map(|err| (err.status, err.code));
                if actual != expected {
                    mismatches.push(format!(
                        "{} (tool: {tool}): expected {expected:?}, got {actual:?}",
                        fixture["label"],
                    ));
                }
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }

    #[test]
    fn responses_keeps_each_text_part_in_history() {
        let fixtures = wire_parity_fixtures();
        let request = decode(
            Surface::Responses,
            json!({"model": "test-model", "input": fixtures["responsesHistory"]}),
            &[connection()],
            Uuid::now_v7(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(request.request.messages).unwrap(),
            json!([
                {"role": "user", "content": [{"type": "text", "text": "First question."}, {"type": "text", "text": "Second question."}]},
                {"role": "assistant", "content": [{"type": "text", "text": "First answer."}, {"type": "text", "text": "Second answer."}]},
                {"role": "user", "content": "Follow-up."}
            ])
        );
    }

    #[test]
    fn responses_accept_stored_files_and_a_final_compaction_trigger() {
        let mut connection = Connection::new(crate::domain::Method::Codex);
        connection.exposed = true;
        connection.models.push(crate::domain::Model {
            id: "test-model".into(),
            name: "Test model".into(),
            reasoning_efforts: vec![],
            default_reasoning_effort: None,
            context_window: 128_000,
            max_output_tokens: 4_000,
            tools: false,
            structured_output: false,
            input_modalities: vec!["text".into()],
        });
        let file_id = format!("file-{}", Uuid::now_v7());
        let request = json!({"model":"test-model", "input":[
            {"type":"compaction","id":"cmp_previous","encrypted_content":"opaque"},
            {"role":"user","content":[{"type":"input_file","file_id":file_id}]},
            {"type":"compaction_trigger"}
        ]});
        let parsed = decode(
            Surface::Responses,
            request.clone(),
            &[connection.clone()],
            Uuid::now_v7(),
        )
        .unwrap();
        let wire = serde_json::to_value(&parsed.request).unwrap();
        assert_eq!(wire["compaction"], true);
        assert_eq!(
            wire["messages"][0]["continuation"]["state"]["items"][0],
            request["input"][0]
        );
        assert_eq!(
            wire["messages"][1]["content"][0]["file"],
            json!({"file_id":file_id})
        );
        assert!(parsed.request.validate(&connection.models[0]).is_ok());

        let mut ambiguous = request.clone();
        ambiguous["input"][1]["content"][0]["filename"] = json!("note.txt");
        ambiguous["input"][1]["content"][0]["file_data"] = json!("data:text/plain;base64,SGVsbG8h");
        let rejected = decode(
            Surface::Responses,
            ambiguous,
            std::slice::from_ref(&connection),
            Uuid::now_v7(),
        );
        assert_eq!(rejected.err().unwrap().code, "INVALID_REQUEST");

        let mut invalid = request;
        invalid["input"].as_array_mut().unwrap().swap(1, 2);
        assert!(decode(Surface::Responses, invalid, &[connection], Uuid::now_v7()).is_err());
    }

    #[tokio::test]
    async fn responses_maps_function_history_and_streams_standard_events() -> anyhow::Result<()> {
        let connection = connection();
        let request_id = Uuid::now_v7();
        let request = decode(
            Surface::Responses,
            json!({
                "model":"test-model", "stream":true,
                "input":[
                    {"type":"function_call","call_id":"old-call","name":"balance","arguments":"{}"},
                    {"type":"function_call_output","call_id":"old-call","output":"25"}
                ],
                "tools":[{"type":"function","name":"balance","parameters":{"type":"object"}}],
                "tool_choice":{"type":"function","name":"balance"}
            }),
            std::slice::from_ref(&connection),
            request_id,
        )
        .unwrap();
        request.request.validate(&connection.models[0]).unwrap();
        assert_eq!(
            request.request.messages[1].tool_call_id.as_deref(),
            Some("old-call")
        );
        let completion = Completion {
            request_id,
            content: String::new(),
            tool_calls: vec![ToolCall::Function {
                id: "new-call".into(),
                function: Invocation {
                    name: "balance".into(),
                    arguments: "{}".into(),
                },
            }],
            usage: Usage {
                input_tokens: 10,
                output_tokens: 2,
                cached_input_tokens: None,
                cache_write_tokens: None,
            },
            provider: "codex".into(),
            method: "codex_http".into(),
            model: "test-model".into(),
            reasoning_effort: None,
            continuation: None,
        };
        request.request.evaluate(&completion, &connection).unwrap();
        let response = response(&request, &completion);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let body = to_bytes(response.into_body(), 64_000).await?;
        let data: Vec<Value> = std::str::from_utf8(&body)?
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?;
        assert_eq!(data.first().unwrap()["type"], "response.created");
        assert!(data.iter().any(
            |event| event["type"] == "response.function_call_arguments.delta"
                && event["delta"] == "{}"
        ));
        let finish = &data.last().unwrap()["response"];
        assert_eq!(finish["status"], "completed");
        assert_eq!(finish["output"][0]["call_id"], "new-call");
        assert_eq!(finish["usage"]["total_tokens"], 12);
        Ok(())
    }

    #[tokio::test]
    async fn chat_includes_requested_stream_usage_and_terminal_frame() -> anyhow::Result<()> {
        let connection = connection();
        let request_id = Uuid::now_v7();
        let request = decode(
            Surface::Chat,
            json!({
                "model":"test-model", "stream":true, "stream_options":{"include_usage":true},
                "messages":[{"role":"user","content":"Hello"}]
            }),
            &[connection],
            request_id,
        )
        .unwrap();
        let completion = Completion {
            request_id,
            content: "Hello".into(),
            tool_calls: vec![],
            usage: Usage {
                input_tokens: 2,
                output_tokens: 1,
                cached_input_tokens: None,
                cache_write_tokens: None,
            },
            provider: "codex".into(),
            method: "codex_http".into(),
            model: "test-model".into(),
            reasoning_effort: None,
            continuation: None,
        };
        let response = response(&request, &completion);
        let body = to_bytes(response.into_body(), 64_000).await?;
        let body = std::str::from_utf8(&body)?;
        assert!(body.contains("\"delta\":{\"content\":\"Hello\"}"));
        assert!(body.contains("\"choices\":[],") && body.contains("\"total_tokens\":3"));
        assert!(body.ends_with("data: [DONE]\n\n"));
        Ok(())
    }
}
