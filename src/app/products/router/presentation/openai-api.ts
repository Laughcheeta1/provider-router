/** OpenAI-compatible Chat Completions and Responses framing over the common provider boundary. */
import type { ServerResponse } from 'node:http';
import { z } from 'zod';
import { inferenceSchema, jsonObjectSchema, messageSchema, RouterError, toolSchema, uuidSchema, type Connection, type InferenceRequest, type InferenceResult, type Message, type Tool } from '../../../common/schemas/contracts.ts';
import { uuid7 } from '../../../common/identity/identity.ts';

const formatSchema = z.object({ type: z.literal('json_schema'), json_schema: z.object({ name: z.string(), schema: jsonObjectSchema, strict: z.boolean().optional() }).strict() }).strict();
const toolChoiceSchema = z.union([z.enum(['auto', 'none', 'required']), z.object({ type: z.literal('function'), function: z.object({ name: z.string() }) })]);
const chatSchema = z.object({ model: z.string().min(1), messages: z.array(messageSchema).min(1), tools: z.array(toolSchema).optional(), response_format: formatSchema.optional(), reasoning_effort: z.string().optional(), max_tokens: z.number().int().positive().optional(), max_completion_tokens: z.number().int().positive().optional(), stream: z.boolean().default(false), stream_options: z.object({ include_usage: z.boolean().optional() }).optional(), tool_choice: toolChoiceSchema.optional(), parallel_tool_calls: z.boolean().optional() }).strict();
const responsesSchema = z.object({ model: z.string().min(1), input: z.union([z.string(), z.array(jsonObjectSchema)]), instructions: z.string().nullable().optional(), tools: z.array(z.object({ type: z.literal('function'), name: z.string(), description: z.string().optional(), parameters: jsonObjectSchema, strict: z.boolean().optional() }).strict()).optional(), text: z.object({ format: z.object({ type: z.literal('json_schema'), name: z.string(), schema: jsonObjectSchema, strict: z.boolean().optional() }).strict() }).optional(), reasoning: z.object({ effort: z.string().nullable().optional() }).optional(), max_output_tokens: z.number().int().positive().optional(), stream: z.boolean().default(false), store: z.literal(false).optional(), tool_choice: z.union([z.enum(['auto', 'none', 'required']), z.object({ type: z.literal('function'), name: z.string() })]).optional(), parallel_tool_calls: z.boolean().optional() }).strict();

export interface OpenAIRequest {
  inference: InferenceRequest;
  tools: Tool[];
  model: string;
  stream: boolean;
  stream_usage: boolean;
  surface: 'chat' | 'responses';
  tool_choice: 'auto' | 'none' | 'required' | { name: string };
  parallel_tool_calls: boolean;
}

/** Qualifying models disambiguates independently exposed connections without custom SDK options. */
export function modelList(connections: Connection[]): Record<string, unknown> {
  return { object: 'list', data: connections.filter((connection: Connection): boolean => connection.exposed).flatMap((connection: Connection): Record<string, unknown>[] => connection.models.map((model): Record<string, unknown> => ({ id: `${connection.id}/${model.id}`, object: 'model', created: 0, owned_by: connection.method === 'openrouter_http' ? 'openrouter' : 'openai', provider_router: { connection_id: connection.id, connection_name: connection.name, provider: connection.method === 'openrouter_http' ? 'openrouter' : 'codex', model: model.id, method: connection.method, name: model.name, reasoning_efforts: model.reasoning_efforts, default_reasoning_effort: model.default_reasoning_effort, context_window: model.context_window, max_output_tokens: model.max_output_tokens, tools: model.tools, structured_output: model.structured_output, input_modalities: model.input_modalities } }))) };
}

/** Decode supported SDK inputs explicitly; unsupported options fail before spending provider tokens. */
export function decodeOpenAI(surface: 'chat' | 'responses', value: unknown, connections: Connection[], requestId?: string): OpenAIRequest {
  const correlation = uuidSchema.safeParse(requestId);
  const id: string = correlation.success ? correlation.data : uuid7();
  if (surface === 'chat') {
    const input = chatSchema.parse(value);
    if (input.max_tokens && input.max_completion_tokens) throw new RouterError('INVALID_REQUEST', 'Specify only one output token limit.');
    const selected: { connection: Connection; model: string; qualified: string } = selectModel(input.model, connections);
    const choice: OpenAIRequest['tool_choice'] = typeof input.tool_choice === 'object' ? { name: input.tool_choice.function.name } : input.tool_choice || 'auto';
    const inference: InferenceRequest = inferenceSchema.parse({ request_id: id, connection_id: selected.connection.id, model: selected.model, messages: input.messages, tools: choice === 'none' ? [] : input.tools || [], reasoning_effort: input.reasoning_effort, response_format: input.response_format, max_output_tokens: input.max_completion_tokens || input.max_tokens, tool_choice: input.tool_choice, parallel_tool_calls: input.parallel_tool_calls });
    validateChoice(choice, inference.tools);
    return { inference, tools: input.tools || [], model: selected.qualified, stream: input.stream, stream_usage: input.stream_options?.include_usage || false, surface, tool_choice: choice, parallel_tool_calls: input.parallel_tool_calls ?? true };
  }
  const input = responsesSchema.parse(value);
  const selected: { connection: Connection; model: string; qualified: string } = selectModel(input.model, connections);
  const messages: Message[] = responseMessages(input.input);
  if (input.instructions) messages.unshift({ role: 'system', content: input.instructions });
  const choice: OpenAIRequest['tool_choice'] = typeof input.tool_choice === 'object' ? { name: input.tool_choice.name } : input.tool_choice || 'auto';
  const tools: Tool[] = (input.tools || []).map((tool): Tool => ({ type: 'function', function: { name: tool.name, description: tool.description, parameters: tool.parameters, strict: tool.strict } }));
  const inference: InferenceRequest = inferenceSchema.parse({ request_id: id, connection_id: selected.connection.id, model: selected.model, messages, tools: choice === 'none' ? [] : (input.tools || []).map((tool): Tool => ({ type: 'function', function: { name: tool.name, description: tool.description, parameters: tool.parameters, strict: tool.strict } })), reasoning_effort: input.reasoning?.effort || undefined, response_format: input.text?.format ? { type: 'json_schema', json_schema: { name: input.text.format.name, schema: input.text.format.schema, strict: input.text.format.strict } } : undefined, max_output_tokens: input.max_output_tokens, tool_choice: typeof choice === 'object' ? { type: 'function', function: { name: choice.name } } : choice, parallel_tool_calls: input.parallel_tool_calls });
  validateChoice(choice, inference.tools);
  return { inference, tools, model: selected.qualified, stream: input.stream, stream_usage: true, surface, tool_choice: choice, parallel_tool_calls: input.parallel_tool_calls ?? true };
}

/** Validate the caller's requested decision shape even for adapters without native choice support. */
export function validateOpenAIResult(request: OpenAIRequest, result: InferenceResult): void {
  if (request.tool_choice === 'required' && !result.tool_calls.length) throw new RouterError('TOOL_CHOICE_UNFULFILLED', 'The provider did not return a required tool call.', 502);
  const choice: OpenAIRequest['tool_choice'] = request.tool_choice;
  if (typeof choice === 'object' && (!result.tool_calls.length || result.tool_calls.some((call): boolean => call.function.name !== choice.name))) throw new RouterError('TOOL_CHOICE_UNFULFILLED', 'The provider did not return the selected tool call.', 502);
  if (!request.parallel_tool_calls && result.tool_calls.length > 1) throw new RouterError('PARALLEL_TOOLS_UNSUPPORTED', 'The provider returned more than one tool call.', 502);
}

/** Emit normal SDK objects or finite SSE streams; version 1 buffers inference until completion. */
export function writeOpenAI(response: ServerResponse, request: OpenAIRequest, result: InferenceResult): void {
  const created: number = Math.floor(Date.now() / 1000);
  const provenance: Record<string, unknown> = { provider: result.provider, method: result.method, connection_id: request.inference.connection_id, model: result.model, ...(result.reasoning_effort ? { reasoning_effort: result.reasoning_effort } : {}) };
  if (request.surface === 'chat') {
    const id: string = `chatcmpl-${result.request_id}`;
    const usage: Record<string, number> = { prompt_tokens: result.usage.input_tokens, completion_tokens: result.usage.output_tokens, total_tokens: result.usage.input_tokens + result.usage.output_tokens };
    const message: Record<string, unknown> = { role: 'assistant', content: result.content || null, ...(result.tool_calls.length ? { tool_calls: result.tool_calls } : {}) };
    const finish: string = result.tool_calls.length ? 'tool_calls' : 'stop';
    if (!request.stream) { json(response, { id, object: 'chat.completion', created, model: request.model, choices: [{ index: 0, message, finish_reason: finish, logprobs: null }], usage, provider_router: provenance }); return; }
    sseHeaders(response);
    const chunk: (delta: Record<string, unknown>, finishReason?: string) => Record<string, unknown> = (delta: Record<string, unknown>, finishReason?: string): Record<string, unknown> => ({ id, object: 'chat.completion.chunk', created, model: request.model, choices: [{ index: 0, delta, finish_reason: finishReason || null, logprobs: null }], provider_router: provenance });
    sse(response, chunk({ role: 'assistant', content: '' }));
    if (result.content) sse(response, chunk({ content: result.content }));
    if (result.tool_calls.length) sse(response, chunk({ tool_calls: result.tool_calls.map((call, index: number) => ({ index, ...call })) }));
    sse(response, chunk({}, finish));
    if (request.stream_usage) sse(response, { id, object: 'chat.completion.chunk', created, model: request.model, choices: [], usage });
    response.end('data: [DONE]\n\n'); return;
  }
  const id: string = `resp_${result.request_id}`;
  const output: Record<string, unknown>[] = result.tool_calls.map((call): Record<string, unknown> => ({ type: 'function_call', id: `fc_${uuid7()}`, call_id: call.id, name: call.function.name, arguments: call.function.arguments, status: 'completed' }));
  if (result.content) output.unshift({ type: 'message', id: `msg_${uuid7()}`, role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: result.content, annotations: [], logprobs: [] }] });
  const object: Record<string, unknown> = { id, object: 'response', created_at: created, status: 'completed', error: null, incomplete_details: null, model: request.model, output, usage: { input_tokens: result.usage.input_tokens, output_tokens: result.usage.output_tokens, total_tokens: result.usage.input_tokens + result.usage.output_tokens, input_tokens_details: { cached_tokens: 0 }, output_tokens_details: { reasoning_tokens: 0 } }, parallel_tool_calls: request.parallel_tool_calls, tool_choice: typeof request.tool_choice === 'object' ? { type: 'function', name: request.tool_choice.name } : request.tool_choice, tools: request.tools.map((tool: Tool) => ({ type: 'function', ...tool.function })), metadata: {}, store: false, provider_router: provenance };
  if (!request.stream) { json(response, object); return; }
  sseHeaders(response);
  let sequence: number = 0;
  const emit: (type: string, body: Record<string, unknown>) => void = (type: string, body: Record<string, unknown>): void => { sse(response, { type, sequence_number: sequence++, ...body }, type); };
  emit('response.created', { response: { ...object, status: 'in_progress', output: [], usage: null } });
  emit('response.in_progress', { response: { ...object, status: 'in_progress', output: [], usage: null } });
  output.forEach((item: Record<string, unknown>, index: number): void => {
    emit('response.output_item.added', { output_index: index, item: { ...item, status: 'in_progress', ...(item.type === 'message' ? { content: [] } : { arguments: '' }) } });
    if (item.type === 'message') {
      const part: Record<string, unknown> = { type: 'output_text', text: result.content, annotations: [], logprobs: [] };
      emit('response.content_part.added', { item_id: item.id, output_index: index, content_index: 0, part: { ...part, text: '' } });
      emit('response.output_text.delta', { item_id: item.id, output_index: index, content_index: 0, delta: result.content, logprobs: [] });
      emit('response.output_text.done', { item_id: item.id, output_index: index, content_index: 0, text: result.content, logprobs: [] });
      emit('response.content_part.done', { item_id: item.id, output_index: index, content_index: 0, part });
    } else {
      emit('response.function_call_arguments.delta', { item_id: item.id, output_index: index, delta: item.arguments });
      emit('response.function_call_arguments.done', { item_id: item.id, output_index: index, arguments: item.arguments, name: item.name });
    }
    emit('response.output_item.done', { output_index: index, item });
  });
  emit('response.completed', { response: object }); response.end();
}

function selectModel(name: string, connections: Connection[]): { connection: Connection; model: string; qualified: string } {
  const choices: { connection: Connection; model: string; qualified: string }[] = [];
  for (const connection of connections) { if (!connection.exposed) continue; for (const model of connection.models) { const qualified: string = `${connection.id}/${model.id}`; if (name === qualified || name === model.id) choices.push({ connection, model: model.id, qualified }); } }
  if (!choices.length) throw new RouterError('MODEL_UNAVAILABLE', 'This model is not exposed by Provider Router.', 404);
  if (choices.length !== 1) throw new RouterError('MODEL_AMBIGUOUS', 'Multiple connections expose this model. Use the qualified model identifier from /v1/models.');
  return choices[0]!;
}

function responseMessages(input: string | Record<string, unknown>[]): Message[] {
  if (typeof input === 'string') return [{ role: 'user', content: input }];
  return input.map((item: Record<string, unknown>): Message => {
    if (item.type === 'function_call') {
      const call = z.object({ type: z.literal('function_call'), call_id: z.string(), name: z.string(), arguments: z.string(), id: z.string().optional(), status: z.string().optional() }).parse(item);
      return { role: 'assistant', content: '', tool_calls: [{ id: call.call_id, type: 'function', function: { name: call.name, arguments: call.arguments } }] };
    }
    if (item.type === 'function_call_output') {
      const output = z.object({ type: z.literal('function_call_output'), call_id: z.string(), output: z.string(), id: z.string().optional() }).parse(item);
      return { role: 'tool', tool_call_id: output.call_id, content: output.output };
    }
    const message = z.object({ role: z.enum(['system', 'developer', 'user', 'assistant']), type: z.literal('message').optional(), content: z.union([z.string(), z.array(z.object({ type: z.enum(['input_text', 'output_text']), text: z.string(), annotations: z.array(z.unknown()).optional(), logprobs: z.array(z.unknown()).optional() }))]), id: z.string().optional(), status: z.string().optional() }).strict().parse(item);
    return { role: message.role, content: typeof message.content === 'string' ? message.content : message.content.map((part): string => part.text).join('\n') };
  });
}

function validateChoice(choice: OpenAIRequest['tool_choice'], tools: Tool[]): void {
  if (choice === 'required' && !tools.length) throw new RouterError('INVALID_TOOL_CHOICE', 'Required tool choice needs at least one function.');
  if (typeof choice === 'object' && !tools.some((tool: Tool): boolean => tool.function.name === choice.name)) throw new RouterError('INVALID_TOOL_CHOICE', 'The selected function is not present in tools.');
}
function json(response: ServerResponse, body: unknown): void { response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }); response.end(JSON.stringify(body)); }
function sseHeaders(response: ServerResponse): void { response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache, no-store', Connection: 'keep-alive', 'X-Accel-Buffering': 'no' }); }
function sse(response: ServerResponse, data: unknown, event?: string): void { response.write(`${event ? `event: ${event}\n` : ''}data: ${JSON.stringify(data)}\n\n`); }
