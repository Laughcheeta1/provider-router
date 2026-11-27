//! Provider-neutral request contracts and validation at the HTTP boundary.
use super::failure::{Failure, Outcome};
use crate::domain::{Connection, Model};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use uuid::Uuid;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub request_id: Uuid,
    pub connection_id: Uuid,
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Format>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compaction: Option<bool>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: Role,
    pub content: Content,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<Continuation>,
}
/// Provider-owned assistant state, scoped to the connection and model that produced it.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Continuation {
    pub connection_id: Uuid,
    pub model: String,
    pub state: ContinuationState,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContinuationState {
    CodexResponses {
        items: Vec<Value>,
    },
    OpenrouterChat {
        message: Map<String, Value>,
        upstream: String,
        resolved_model: String,
    },
}
impl ContinuationState {
    /// Return the opaque item only when the continuation is a compaction checkpoint.
    pub fn compaction_item(&self) -> Option<&Value> {
        let Self::CodexResponses { items } = self else {
            return None;
        };
        let [item] = items.as_slice() else {
            return None;
        };
        is_compaction_item(item).then_some(item)
    }
}

pub fn is_compaction_item(item: &Value) -> bool {
    item["type"] == "compaction"
        && item["encrypted_content"]
            .as_str()
            .is_some_and(|content| !content.is_empty())
}

impl Message {
    pub fn text(role: Role, text: String) -> Self {
        Self {
            role,
            content: Content::Text(text),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            continuation: None,
        }
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<Part>),
    Null,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Part {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prompt_cache_boundary: Option<bool>,
    },
    File {
        file: Attachment,
    },
    ImageUrl {
        image_url: Image,
    },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Attachment {
    Inline { filename: String, file_data: String },
    Reference { file_id: String },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub url: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Tool {
    Function { function: Function },
}
impl Tool {
    pub fn function(&self) -> &Function {
        let Self::Function { function } = self;
        function
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Function {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolCall {
    Function { id: String, function: Invocation },
}
impl ToolCall {
    pub fn parts(&self) -> (&str, &Invocation) {
        let Self::Function { id, function } = self;
        (id, function)
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub name: String,
    pub arguments: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Format {
    JsonSchema { json_schema: Schema },
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Schema {
    pub name: String,
    pub schema: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}
#[derive(Deserialize, Serialize)]
pub struct Completion {
    pub request_id: Uuid,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub provider: String,
    pub method: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continuation: Option<Continuation>,
}
#[derive(Deserialize, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
}
impl Request {
    pub fn validate(&self, model: &Model) -> Outcome<()> {
        if [self.request_id, self.connection_id]
            .iter()
            .any(|id| id.get_version_num() != 7)
            || self.model.is_empty()
            || self.messages.is_empty()
            || self.session_id.is_some_and(|id| id.get_version_num() != 7)
        {
            return Err(Failure::invalid());
        }
        if let Some(effort) = &self.reasoning_effort
            && !model.reasoning_efforts.contains(effort)
        {
            return Err(Failure::bad(
                "EFFORT_UNSUPPORTED",
                "The selected reasoning effort is unavailable for this model.",
            ));
        }
        if !self.tools.is_empty() && !model.tools {
            return Err(Failure::bad(
                "TOOLS_UNSUPPORTED",
                "The selected model does not support tools.",
            ));
        }
        if self.response_format.is_some() && !model.structured_output {
            return Err(Failure::bad(
                "SCHEMA_UNSUPPORTED",
                "The selected model does not support structured output.",
            ));
        }
        if self
            .max_output_tokens
            .is_some_and(|max| max == 0 || max > model.max_output_tokens)
        {
            return Err(Failure::bad(
                "OUTPUT_LIMIT",
                "The output token limit exceeds this model’s capability.",
            ));
        }
        let mut calls = HashSet::new();
        let mut answered = HashSet::new();
        for message in &self.messages {
            if let Some(continuation) = &message.continuation
                && (!matches!(message.role, Role::Assistant)
                    || continuation.connection_id != self.connection_id
                    || continuation.model != self.model)
            {
                return Err(Failure::bad(
                    "INVALID_HISTORY",
                    "Assistant continuation belongs to another connection or model.",
                ));
            }
            if let Content::Parts(parts) = &message.content {
                for part in parts {
                    match part {
                        Part::Text { .. } => (),
                        Part::File {
                            file:
                                Attachment::Inline {
                                    filename,
                                    file_data,
                                },
                        } if matches!(message.role, Role::User)
                            && !filename.is_empty()
                            && filename.chars().count() <= 255
                            && !file_data.is_empty() => {}
                        Part::File {
                            file: Attachment::Reference { file_id },
                        } if matches!(message.role, Role::User) => {
                            super::files::file_uuid(file_id)?;
                        }
                        _ => {
                            return Err(Failure::bad(
                                "MEDIA_UNSUPPORTED",
                                "Only text and user document attachments are supported.",
                            ));
                        }
                    }
                }
            }
            if matches!(message.role, Role::Tool)
                && !message
                    .tool_call_id
                    .as_deref()
                    .is_some_and(|id| calls.contains(id) && answered.insert(id))
            {
                return Err(Failure::bad(
                    "INVALID_HISTORY",
                    "Tool results must match one preceding assistant function call.",
                ));
            }
            for call in message.tool_calls.iter().flatten() {
                let (id, function) = call.parts();
                if !matches!(message.role, Role::Assistant)
                    || id.is_empty()
                    || function.name.is_empty()
                    || !calls.insert(id)
                {
                    return Err(Failure::bad(
                        "INVALID_HISTORY",
                        "Assistant function calls must have distinct identifiers.",
                    ));
                }
            }
        }
        if answered.len() != calls.len() {
            return Err(Failure::bad(
                "INVALID_HISTORY",
                "Supply each preceding function result before requesting another inference.",
            ));
        }
        let mut tool_names = HashSet::new();
        for tool in &self.tools {
            let function = tool.function();
            if function.name.is_empty() || !tool_names.insert(&function.name) {
                return Err(Failure::bad(
                    "INVALID_TOOLS",
                    "Tool names must be nonempty and unique.",
                ));
            }
            validate_schema(&function.parameters)?;
        }
        if let Some(Format::JsonSchema { json_schema }) = &self.response_format {
            validate_schema(&json_schema.schema)?;
        }
        let choice = self.tool_choice.as_ref().map(parse_choice).transpose()?;
        match choice.as_ref() {
            Some(Choice::Required) if tool_names.is_empty() => Err(Failure::bad(
                "INVALID_TOOL_CHOICE",
                "Required tool choice needs at least one function.",
            )),
            Some(Choice::Named(name)) if !tool_names.contains(name) => Err(Failure::bad(
                "INVALID_TOOL_CHOICE",
                "The selected function is not present in tools.",
            )),
            _ => Ok(()),
        }
    }
    pub fn evaluate(&self, completion: &Completion, connection: &Connection) -> Outcome<()> {
        if completion.request_id != self.request_id
            || completion.model != self.model
            || completion.method != serde_json::to_value(connection.method).unwrap_or(Value::Null)
            || completion.provider != connection.method.provider()
        {
            return Err(Failure::provider(
                "INVALID_COMPLETION",
                "The provider returned a mismatched inference response.",
            ));
        }
        if self
            .max_output_tokens
            .is_some_and(|max| completion.usage.output_tokens > max)
        {
            return Err(Failure::provider(
                "OUTPUT_LIMIT",
                "The provider exceeded the requested output budget.",
            ));
        }
        if self.compaction == Some(true) {
            if completion.content.is_empty()
                && completion.tool_calls.is_empty()
                && completion
                    .continuation
                    .as_ref()
                    .is_some_and(|continuation| {
                        continuation.connection_id == self.connection_id
                            && continuation.model == self.model
                            && continuation.state.compaction_item().is_some()
                    })
            {
                return Ok(());
            }

            return Err(Failure::provider(
                "INVALID_COMPACTION",
                "The provider did not return a valid compaction checkpoint.",
            ));
        }
        if completion.content.trim().is_empty() && completion.tool_calls.is_empty() {
            return Err(Failure::provider(
                "EMPTY_COMPLETION",
                "The provider returned no answer or tool requests.",
            ));
        }
        if self.parallel_tool_calls == Some(false) && completion.tool_calls.len() > 1 {
            return Err(Failure::provider(
                "PARALLEL_TOOLS_UNSUPPORTED",
                "The provider returned more than one tool call.",
            ));
        }
        let choice = self.tool_choice.as_ref().map(parse_choice).transpose()?;
        match choice {
            Some(Choice::Required) if completion.tool_calls.is_empty() => {
                return Err(Failure::provider(
                    "TOOL_CHOICE_UNFULFILLED",
                    "The provider did not return a required tool call.",
                ));
            }
            Some(Choice::Named(name))
                if completion.tool_calls.is_empty()
                    || completion
                        .tool_calls
                        .iter()
                        .any(|call| call.parts().1.name != name) =>
            {
                return Err(Failure::provider(
                    "TOOL_CHOICE_UNFULFILLED",
                    "The provider did not return the selected tool call.",
                ));
            }
            Some(Choice::None) if !completion.tool_calls.is_empty() => {
                return Err(Failure::provider(
                    "INVALID_TOOL_CALLS",
                    "The provider returned an unavailable tool.",
                ));
            }
            _ => (),
        }
        let mut ids = HashSet::new();
        for call in &completion.tool_calls {
            let (id, function) = call.parts();
            if id.is_empty() || !ids.insert(id) {
                return Err(Failure::provider(
                    "INVALID_TOOL_CALLS",
                    "The provider returned repeated or empty tool call identifiers.",
                ));
            }
            let tool = self
                .tools
                .iter()
                .find(|tool| tool.function().name == function.name)
                .ok_or_else(|| {
                    Failure::provider(
                        "INVALID_TOOL_CALLS",
                        "The provider requested an unavailable tool.",
                    )
                })?;
            if !valid_json(&tool.function().parameters, &function.arguments) {
                return Err(Failure::provider(
                    "INVALID_TOOL_ARGUMENTS",
                    "The provider returned arguments outside the tool contract.",
                ));
            }
        }
        if let Some(Format::JsonSchema { json_schema }) = &self.response_format
            && completion.tool_calls.is_empty()
            && !valid_json(&json_schema.schema, &completion.content)
        {
            return Err(Failure::provider(
                "INVALID_COMPLETION",
                "The provider answer does not match the requested schema.",
            ));
        }
        Ok(())
    }
}
#[derive(Clone)]
pub enum Choice {
    Auto,
    None,
    Required,
    Named(String),
}
pub fn parse_choice(value: &Value) -> Outcome<Choice> {
    match value.as_str() {
        Some("auto") => Ok(Choice::Auto),
        Some("none") => Ok(Choice::None),
        Some("required") => Ok(Choice::Required),
        _ if value["type"] == "function" => value["function"]["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(|name| Choice::Named(name.to_owned()))
            .ok_or_else(Failure::invalid),
        _ => Err(Failure::invalid()),
    }
}
fn validator(schema: &Map<String, Value>) -> Option<jsonschema::Validator> {
    jsonschema::options()
        .should_validate_formats(false)
        .build(&Value::Object(schema.clone()))
        .ok()
}
fn validate_schema(schema: &Map<String, Value>) -> Outcome<()> {
    validator(schema).map(|_| ()).ok_or_else(|| {
        Failure::bad(
            "INVALID_SCHEMA",
            "Tool and response schemas must be valid local JSON schemas.",
        )
    })
}
fn valid_json(schema: &Map<String, Value>, instance: &str) -> bool {
    let Ok(instance) = serde_json::from_str::<Value>(instance) else {
        return false;
    };
    validator(schema).is_some_and(|validator| validator.is_valid(&instance))
}
pub fn has_file(message: &Message) -> bool {
    matches!(&message.content, Content::Parts(parts) if parts.iter().any(|part| matches!(part, Part::File { .. })))
}
