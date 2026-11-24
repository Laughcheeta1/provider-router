//! Provider-neutral request contracts and validation at the HTTP boundary.
// RevExt: 1
use super::failure::{Failure, Outcome};
use crate::domain::{Connection, Model};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use uuid::Uuid;
// RevExt: 2
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
} // RevExt: 31
// RevExt: 3
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
    Tool,
} // RevExt: 32
// RevExt: 4
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
} // RevExt: 33
// RevExt: 5
impl Message {
    pub fn text(role: Role, text: String) -> Self {
        Self {
            role,
            content: Content::Text(text),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }
} // RevExt: 34
// RevExt: 6
#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<Part>),
    Null,
} // RevExt: 35
// RevExt: 7
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Part {
    Text { text: String },
    File { file: Attachment },
    ImageUrl { image_url: Image },
} // RevExt: 36
// RevExt: 8
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    pub filename: String,
    pub file_data: String,
} // RevExt: 37
// RevExt: 9
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub url: String,
} // RevExt: 38
// RevExt: 10
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Tool {
    Function { function: Function },
} // RevExt: 39
// RevExt: 11
impl Tool {
    pub fn function(&self) -> &Function {
        let Self::Function { function } = self;
        function
    }
} // RevExt: 40
// RevExt: 12
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Function {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub parameters: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
} // RevExt: 41
// RevExt: 13
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolCall {
    Function { id: String, function: Invocation },
} // RevExt: 42
// RevExt: 14
impl ToolCall {
    pub fn parts(&self) -> (&str, &Invocation) {
        let Self::Function { id, function } = self;
        (id, function)
    }
} // RevExt: 43
// RevExt: 15
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub name: String,
    pub arguments: String,
} // RevExt: 44
// RevExt: 16
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Format {
    JsonSchema { json_schema: Schema },
} // RevExt: 45
// RevExt: 17
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Schema {
    pub name: String,
    pub schema: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
} // RevExt: 46
// RevExt: 18
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
} // RevExt: 47
// RevExt: 19
#[derive(Deserialize, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
} // RevExt: 48
// RevExt: 20
impl Request {
    pub fn validate(&self, model: &Model) -> Outcome<()> {
        if [self.request_id, self.connection_id]
            .iter()
            .any(|id| id.get_version_num() != 7)
            || self.model.is_empty()
            || self.messages.is_empty()
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
        // RevExt: 21
        let mut calls = HashSet::new();
        let mut answered = HashSet::new();
        for message in &self.messages {
            if let Content::Parts(parts) = &message.content {
                for part in parts {
                    match part {
                        Part::Text { .. } => (),
                        Part::File { file }
                            if matches!(message.role, Role::User)
                                && !file.filename.is_empty()
                                && file.filename.chars().count() <= 255
                                && !file.file_data.is_empty() => {}
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
        // RevExt: 22
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
    // RevExt: 23
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
} // RevExt: 49
// RevExt: 24
#[derive(Clone)]
pub enum Choice {
    Auto,
    None,
    Required,
    Named(String),
} // RevExt: 50
// RevExt: 25
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
} // RevExt: 51
// RevExt: 26
fn validator(schema: &Map<String, Value>) -> Option<jsonschema::Validator> {
    jsonschema::options()
        .should_validate_formats(false)
        .build(&Value::Object(schema.clone()))
        .ok()
} // RevExt: 52
// RevExt: 27
fn validate_schema(schema: &Map<String, Value>) -> Outcome<()> {
    validator(schema).map(|_| ()).ok_or_else(|| {
        Failure::bad(
            "INVALID_SCHEMA",
            "Tool and response schemas must be valid local JSON schemas.",
        )
    })
} // RevExt: 53
// RevExt: 28
fn valid_json(schema: &Map<String, Value>, instance: &str) -> bool {
    let Ok(instance) = serde_json::from_str::<Value>(instance) else {
        return false;
    };
    validator(schema).is_some_and(|validator| validator.is_valid(&instance))
} // RevExt: 54
// RevExt: 29
pub fn has_file(message: &Message) -> bool {
    matches!(&message.content, Content::Parts(parts) if parts.iter().any(|part| matches!(part, Part::File { .. })))
} // RevExt: 55
// RevExt: 30
