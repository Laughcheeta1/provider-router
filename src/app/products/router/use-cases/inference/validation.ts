/** Validate provider answers and tools at the boundary, before returning them to any client. */
import { RouterError, toolChoiceSchema, type InferenceBody, type Continuation, type InferenceRequest, type InferenceResult, type Model, type Tool, type ToolCall } from '../../../../common/schemas/contracts.ts';
import { compileJsonSchema, matchesJsonSchema } from './json-schema.ts';

/** Enforce advertised model choices, output limits and represented input modalities. */
export function validateRequest(request: InferenceBody, models: Model[]): InferenceRequest {
  const model: Model | undefined = models.find((item: Model): boolean => item.id === request.model);
  if (!model) throw new RouterError('MODEL_UNAVAILABLE', 'The selected model is not exposed by this connection.');
  if (!request.messages.length || (request.session_id !== undefined && request.session_id[14] !== '7')) throw new RouterError('INVALID_REQUEST', 'The request does not match the API contract.');
  if (request.reasoning_effort !== undefined && !model.reasoning_efforts.includes(request.reasoning_effort)) throw new RouterError('EFFORT_UNSUPPORTED', 'The selected reasoning effort is unavailable for this model.');
  if (request.tools.length && !model.tools) throw new RouterError('TOOLS_UNSUPPORTED', 'The selected model does not support tools.');
  if (request.response_format && !model.structured_output) throw new RouterError('SCHEMA_UNSUPPORTED', 'The selected model does not support structured output.');
  if (request.max_output_tokens !== undefined && (request.max_output_tokens === 0 || request.max_output_tokens > model.max_output_tokens)) throw new RouterError('OUTPUT_LIMIT', 'The output token limit exceeds this model’s capability.');
  const calls: Set<string> = new Set();
  const answered: Set<string> = new Set();
  for (const message of request.messages) {
    const continuation: Continuation | undefined = message.continuation;
    if (continuation && (message.role !== 'assistant' || continuation.connection_id.toLowerCase() !== request.connection_id.toLowerCase() || continuation.model !== request.model)) {
      throw new RouterError('INVALID_HISTORY', 'Assistant continuation belongs to another connection or model.');
    }
    if (Array.isArray(message.content)) {
      for (const part of message.content) {
        if (part.type === 'text') continue;
        if (message.role === 'user' && part.type === 'file') {
          const file: { filename: string; file_data: string } = part.file as { filename: string; file_data: string };
          if (file.filename && [...file.filename].length <= 255 && file.file_data) continue;
        }
        throw new RouterError('MEDIA_UNSUPPORTED', 'Only text and user document attachments are supported.');
      }
    }
    if (message.role === 'tool') {
      if (!message.tool_call_id || !calls.has(message.tool_call_id) || answered.has(message.tool_call_id)) throw new RouterError('INVALID_HISTORY', 'Tool results must match one preceding assistant function call.');
      answered.add(message.tool_call_id);
    }
    if (message.tool_calls?.length && message.role !== 'assistant') throw new RouterError('INVALID_HISTORY', 'Only assistant messages may contain function calls.');
    for (const call of message.tool_calls || []) { if (!call.id || !call.function.name || calls.has(call.id)) throw new RouterError('INVALID_HISTORY', 'Function call identifiers must be distinct.'); calls.add(call.id); }
  }
  if (answered.size !== calls.size) throw new RouterError('INVALID_HISTORY', 'Supply each preceding function result before requesting another inference.');
  const toolNames: Set<string> = new Set();
  for (const tool of request.tools) {
    const { name, parameters } = tool.function;
    if (!name || toolNames.has(name)) throw new RouterError('INVALID_TOOLS', 'Tool names must be nonempty and unique.');
    toolNames.add(name);
    validateSchema(parameters);
  }
  if (request.response_format) validateSchema(request.response_format.json_schema.schema);
  const choice: InferenceRequest['tool_choice'] = toolChoiceSchema.parse(request.tool_choice);
  if (choice === 'required' && !toolNames.size) throw new RouterError('INVALID_TOOL_CHOICE', 'Required tool choice needs at least one function.');
  if (typeof choice === 'object' && !toolNames.has(choice.function.name)) throw new RouterError('INVALID_TOOL_CHOICE', 'The selected function is not present in tools.');
  return { ...request, tool_choice: choice };
}

/** A completed inference returns either an answer or tool requests, with valid JSON arguments. */
export function validateResult(request: InferenceRequest, result: InferenceResult): InferenceResult {
  if (request.max_output_tokens !== undefined && result.usage.output_tokens > request.max_output_tokens) throw new RouterError('OUTPUT_LIMIT', 'The provider exceeded the requested output budget.', 502);
  if (!result.content.trim() && !result.tool_calls.length) throw new RouterError('EMPTY_COMPLETION', 'The provider returned no answer or tool requests.', 502);
  if (request.parallel_tool_calls === false && result.tool_calls.length > 1) throw new RouterError('PARALLEL_TOOLS_UNSUPPORTED', 'The provider returned more than one tool call.', 502);
  const choice: InferenceRequest['tool_choice'] = request.tool_choice;
  if (choice === 'required' && !result.tool_calls.length) throw new RouterError('TOOL_CHOICE_UNFULFILLED', 'The provider did not return a required tool call.', 502);
  if (typeof choice === 'object' && (!result.tool_calls.length || result.tool_calls.some((call: ToolCall): boolean => call.function.name !== choice.function.name))) throw new RouterError('TOOL_CHOICE_UNFULFILLED', 'The provider did not return the selected tool call.', 502);
  if (choice === 'none' && result.tool_calls.length) throw new RouterError('INVALID_TOOL_CALLS', 'The provider returned an unavailable tool.', 502);
  const ids: string[] = result.tool_calls.map((call: ToolCall): string => call.id);
  if (ids.includes('') || new Set(ids).size !== ids.length) throw new RouterError('INVALID_TOOL_CALLS', 'The provider returned repeated or empty tool call identifiers.', 502);
  for (const call of result.tool_calls) {
    const tool: Tool | undefined = request.tools.find((item: Tool): boolean => item.function.name === call.function.name);
    if (!tool) throw new RouterError('INVALID_TOOL_CALLS', 'The provider requested an unavailable tool.', 502);
    if (!matchesJsonSchema(tool.function.parameters, call.function.arguments)) throw new RouterError('INVALID_TOOL_ARGUMENTS', 'The provider returned arguments outside the tool contract.', 502);
  }
  if (!result.tool_calls.length && request.response_format) {
    if (!matchesJsonSchema(request.response_format.json_schema.schema, result.content)) throw new RouterError('INVALID_COMPLETION', 'The provider answer does not match the requested schema.', 502);
  }
  return result;
}

function validateSchema(schema: Record<string, unknown>): void {
  try { compileJsonSchema(schema); }
  catch { throw new RouterError('INVALID_SCHEMA', 'Tool and response schemas must be valid local JSON schemas.'); }
}
