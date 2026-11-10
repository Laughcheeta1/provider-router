/** Validate provider answers and tools at the boundary, before returning them to any client. */
import { Ajv } from 'ajv';
import { RouterError, type InferenceRequest, type InferenceResult, type Model, type Tool, type ToolCall } from '../../../../common/schemas/contracts.ts';

const ajv: Ajv = new Ajv({ strict: false, allErrors: false, validateFormats: false });
/** Enforce advertised model choices, output limits and represented input modalities. */
export function validateRequest(request: InferenceRequest, models: Model[]): void {
  const model: Model | undefined = models.find((item: Model): boolean => item.id === request.model);
  if (!model) throw new RouterError('MODEL_UNAVAILABLE', 'The selected model is not exposed by this connection.');
  if (request.reasoning_effort && !model.reasoning_efforts.includes(request.reasoning_effort)) throw new RouterError('EFFORT_UNSUPPORTED', 'The selected reasoning effort is unavailable for this model.');
  if (request.tools.length && !model.tools) throw new RouterError('TOOLS_UNSUPPORTED', 'The selected model does not support tools.');
  if (request.response_format && !model.structured_output) throw new RouterError('SCHEMA_UNSUPPORTED', 'The selected model does not support structured output.');
  if (request.max_output_tokens && request.max_output_tokens > model.max_output_tokens) throw new RouterError('OUTPUT_LIMIT', 'The output token limit exceeds this model’s capability.');
  // Version 1 rejects media rather than flattening or silently dropping content blocks.
  if (request.messages.some((message): boolean => Array.isArray(message.content) && message.content.some((part): boolean => part.type !== 'text' || typeof part.text !== 'string'))) throw new RouterError('MEDIA_UNSUPPORTED', 'This adapter currently accepts text messages only.');
  const calls: Set<string> = new Set();
  const answered: Set<string> = new Set();
  for (const message of request.messages) {
    if (message.role === 'tool') {
      if (!message.tool_call_id || !calls.has(message.tool_call_id) || answered.has(message.tool_call_id)) throw new RouterError('INVALID_HISTORY', 'Tool results must match one preceding assistant function call.');
      answered.add(message.tool_call_id);
    }
    if (message.tool_calls?.length && message.role !== 'assistant') throw new RouterError('INVALID_HISTORY', 'Only assistant messages may contain function calls.');
    for (const call of message.tool_calls || []) { if (calls.has(call.id)) throw new RouterError('INVALID_HISTORY', 'Function call identifiers must be distinct.'); calls.add(call.id); }
  }
  if (answered.size !== calls.size) throw new RouterError('INVALID_HISTORY', 'Supply each preceding function result before requesting another inference.');
  const toolNames: string[] = request.tools.map((tool: Tool): string => tool.function.name);
  if (new Set(toolNames).size !== toolNames.length) throw new RouterError('INVALID_TOOLS', 'Tool names must be unique.');
}

/** A completed inference returns either an answer or tool requests, with valid JSON arguments. */
export function validateResult(request: InferenceRequest, result: InferenceResult): InferenceResult {
  if (request.max_output_tokens && result.usage.output_tokens > request.max_output_tokens) throw new RouterError('OUTPUT_LIMIT', 'The provider exceeded the requested output budget.', 502);
  if (!result.content.trim() && !result.tool_calls.length) throw new RouterError('EMPTY_COMPLETION', 'The provider returned no answer or tool requests.', 502);
  const ids: string[] = result.tool_calls.map((call: ToolCall): string => call.id);
  if (new Set(ids).size !== ids.length) throw new RouterError('INVALID_TOOL_CALLS', 'The provider returned repeated tool call identifiers.', 502);
  for (const call of result.tool_calls) {
    const tool: Tool | undefined = request.tools.find((item: Tool): boolean => item.function.name === call.function.name);
    if (!tool) throw new RouterError('INVALID_TOOL_CALLS', 'The provider requested an unavailable tool.', 502);
    let args: unknown;
    try { args = JSON.parse(call.function.arguments) as unknown; }
    catch { throw new RouterError('INVALID_TOOL_CALLS', 'The provider returned invalid tool arguments.', 502); }
    if (!ajv.validate(tool.function.parameters, args)) throw new RouterError('INVALID_TOOL_ARGUMENTS', 'The provider returned arguments outside the tool contract.', 502);
  }
  if (!result.tool_calls.length && request.response_format) {
    let answer: unknown;
    try { answer = JSON.parse(result.content) as unknown; }
    catch { throw new RouterError('INVALID_COMPLETION', 'The provider returned invalid structured output.', 502); }
    if (!ajv.validate(request.response_format.json_schema.schema, answer)) throw new RouterError('INVALID_COMPLETION', 'The provider answer does not match the requested schema.', 502);
  }
  return result;
}
