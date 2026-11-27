/** OpenAI-compatible Chat Completions and Responses framing over the common provider boundary. */
import type { ServerResponse } from 'node:http';
import { z } from 'zod';
import { formatSchema, functionSchema, inferenceIdSchema, nativeUuidSchema, messageSchema, roleSchema, RouterError, structuredSchema, toolChoiceSchema, toolSchema, type Connection, type InferenceRequest, type InferenceResult, type Message, type Tool } from '../../../common/schemas/contracts.ts';
import { stringifyWireJson } from '../../../common/schemas/json-wire.ts';
import { nativeOption, nativeStruct } from '../../../common/schemas/native-struct.ts';
import { tokenCountSchema, totalTokenCount, type TokenCount } from '../../../common/schemas/token-count.ts';
import { uuid7 } from '../../../common/identity/identity.ts';
const chatSchema = nativeStruct(z.strictObject({
  model: z.string(),
  messages: z.array(messageSchema),
  tools: z.array(toolSchema).default([]),
  response_format: nativeOption(formatSchema),
  reasoning_effort: nativeOption(z.string()),
  max_tokens: nativeOption(tokenCountSchema),
  max_completion_tokens: nativeOption(tokenCountSchema),
  stream: z.boolean().default(false),
  stream_options: nativeOption(nativeStruct(z.strictObject({ include_usage: z.boolean().default(false) }), undefined, 0)),
  tool_choice: z.unknown().optional(),
  parallel_tool_calls: nativeOption(z.boolean()),
}), undefined, undefined, ['max_tokens', 'max_completion_tokens']);
const responseTextPartSchema = z.object({ type: z.enum(['input_text', 'output_text']), text: z.string() });
const responseFilePartSchema = z.object({ type: z.literal('input_file'), filename: z.string(), file_data: z.string() });
const responseInputSchema = z.union([z.string(), z.array(z.union([
  z.object({ type: z.literal('function_call'), call_id: z.string(), name: z.string(), arguments: z.string(), id: z.string().nullish(), status: z.string().nullish() }).strict(),
  z.object({ type: z.literal('function_call_output'), call_id: z.string(), output: z.string(), id: z.string().nullish() }).strict(),
  nativeStruct(z.strictObject({ role: roleSchema, type: z.string().nullish(), content: z.union([z.string(), z.array(z.union([responseTextPartSchema, responseFilePartSchema]))]), id: z.string().nullish(), status: z.string().nullish() })),
]))]);
const responsesSchema = nativeStruct(z.strictObject({
  model: z.string(),
  input: responseInputSchema,
  instructions: nativeOption(z.string()),
  tools: z.array(functionSchema.extend({ type: z.literal('function') })).default([]),
  text: nativeOption(nativeStruct(z.strictObject({ format: structuredSchema.extend({ type: z.literal('json_schema') }) }))),
  reasoning: nativeOption(nativeStruct(z.strictObject({ effort: nativeOption(z.string()) }))),
  max_output_tokens: nativeOption(tokenCountSchema),
  stream: z.boolean().default(false),
  store: nativeOption(z.boolean()),
  tool_choice: z.unknown().optional(),
  parallel_tool_calls: nativeOption(z.boolean()),
  provider_router_messages: nativeOption(z.array(messageSchema)),
  session_id: nativeOption(nativeUuidSchema),
}), undefined, undefined, ['max_output_tokens']);
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
  const correlation = inferenceIdSchema.safeParse(requestId);
  const id: string = correlation.success ? correlation.data : uuid7();
  if (surface === 'chat') {
    const input = chatSchema.parse(value);
    if (input.max_tokens !== undefined && input.max_completion_tokens !== undefined) throw new RouterError('INVALID_REQUEST', 'The request does not match the API contract.');
    const selected: ReturnType<typeof selectModel> = selectModel(input.model, connections);
    const toolChoice: InferenceRequest['tool_choice'] = toolChoiceSchema.parse(input.tool_choice);
    const tools: Tool[] = toolChoice === 'none' ? [] : input.tools;
    const inference: InferenceRequest = {
      request_id: id, connection_id: selected.connection.id, model: selected.model,
      messages: input.messages, tools, reasoning_effort: input.reasoning_effort,
      response_format: input.response_format, max_output_tokens: input.max_completion_tokens ?? input.max_tokens,
      tool_choice: toolChoice, parallel_tool_calls: input.parallel_tool_calls,
    };
    return { inference, tools, model: selected.qualified, stream: input.stream, stream_usage: input.stream_options?.include_usage ?? false, surface, tool_choice: openAIChoice(toolChoice), parallel_tool_calls: input.parallel_tool_calls ?? true };
  }
  const input = responsesSchema.parse(value);
  if (input.store) throw new RouterError('INVALID_REQUEST', 'The request does not match the API contract.');
  const selected: ReturnType<typeof selectModel> = selectModel(input.model, connections);
  const messages: Message[] = input.provider_router_messages ?? responseMessages(input.input);
  if (input.instructions !== undefined) messages.unshift({ role: 'system', content: input.instructions });
  const rawChoice: unknown = input.tool_choice;
  const toolChoice: InferenceRequest['tool_choice'] = toolChoiceSchema.parse(
    rawChoice && typeof rawChoice === 'object' && 'type' in rawChoice && rawChoice.type === 'function'
      ? { type: 'function', function: { name: 'name' in rawChoice ? rawChoice.name : undefined } }
      : rawChoice,
  );
  const tools: Tool[] = toolChoice === 'none' ? [] : input.tools.map(({ type, ...fn }): Tool => ({ type, function: fn }));
  const format = input.text?.format;
  const inference: InferenceRequest = {
    request_id: id, connection_id: selected.connection.id, model: selected.model, messages, tools,
    reasoning_effort: input.reasoning?.effort,
    response_format: format && { type: format.type, json_schema: { name: format.name, schema: format.schema, strict: format.strict } },
    max_output_tokens: input.max_output_tokens, tool_choice: toolChoice,
    parallel_tool_calls: input.parallel_tool_calls, session_id: input.session_id,
  };
  return { inference, tools, model: selected.qualified, stream: input.stream, stream_usage: true, surface, tool_choice: openAIChoice(toolChoice), parallel_tool_calls: input.parallel_tool_calls ?? true };
}

function openAIChoice(choice: InferenceRequest['tool_choice']): OpenAIRequest['tool_choice'] {
  return typeof choice === 'object' ? { name: choice.function.name } : choice ?? 'auto';
}

/** Emit normal SDK objects or finite SSE streams; version 1 buffers inference until completion. */
export function writeOpenAI(response: ServerResponse, request: OpenAIRequest, result: InferenceResult): void {
  const created: number = Math.floor(Date.now() / 1000);
  const provenance: Record<string, unknown> = { provider: result.provider, method: result.method, connection_id: request.inference.connection_id, model: result.model, ...(result.reasoning_effort !== undefined ? { reasoning_effort: result.reasoning_effort } : {}) };
  const { input_tokens, output_tokens, cached_input_tokens, cache_write_tokens } = result.usage;
  const total: TokenCount = totalTokenCount(input_tokens, output_tokens);
  const cacheDetails: Record<string, TokenCount> = {};
  if (cached_input_tokens !== undefined) cacheDetails.cached_tokens = cached_input_tokens;
  if (cache_write_tokens !== undefined) cacheDetails.cache_write_tokens = cache_write_tokens;
  if (request.surface === 'chat') {
    const id: string = `chatcmpl-${result.request_id}`;
    const usage: Record<string, unknown> = { prompt_tokens: input_tokens, completion_tokens: output_tokens, total_tokens: total, prompt_tokens_details: cacheDetails };
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
  if (!request.stream && result.continuation) provenance.continuation = result.continuation;
  const id: string = `resp_${result.request_id}`;
  const output: Record<string, unknown>[] = result.tool_calls.map((call): Record<string, unknown> => ({ type: 'function_call', id: `fc_${uuid7()}`, call_id: call.id, name: call.function.name, arguments: call.function.arguments, status: 'completed' }));
  if (result.content) output.unshift({ type: 'message', id: `msg_${uuid7()}`, role: 'assistant', status: 'completed', content: [{ type: 'output_text', text: result.content, annotations: [], logprobs: [] }] });
  const usage: Record<string, unknown> = { input_tokens, output_tokens, total_tokens: total, ...(Object.keys(cacheDetails).length ? { input_tokens_details: cacheDetails } : {}) };
  const object: Record<string, unknown> = { id, object: 'response', created_at: created, status: 'completed', error: null, incomplete_details: null, model: request.model, output, usage, parallel_tool_calls: request.parallel_tool_calls, tool_choice: typeof request.tool_choice === 'object' ? { type: 'function', name: request.tool_choice.name } : request.tool_choice, tools: request.tools.map((tool: Tool) => ({ type: 'function', ...tool.function })), metadata: {}, store: false, provider_router: provenance };
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
function responseMessages(input: z.infer<typeof responseInputSchema>): Message[] {
  if (typeof input === 'string') return [{ role: 'user', content: input }];
  return input.map((item): Message => {
    if ('role' in item) {
      if (item.role === 'tool' || (item.type != null && item.type !== 'message')) throw new RouterError('INVALID_REQUEST', 'The request does not match the API contract.');
      if (typeof item.content === 'string') return { role: item.role, content: item.content };
      return { role: item.role, content: item.content.map((part): Record<string, unknown> => part.type === 'input_file'
        ? { type: 'file', file: { filename: part.filename, file_data: part.file_data } }
        : { type: 'text', text: part.text }) };
    }
    if (item.type === 'function_call') {
      return { role: 'assistant', content: '', tool_calls: [{ id: item.call_id, type: 'function', function: { name: item.name, arguments: item.arguments } }] };
    }
    return { role: 'tool', tool_call_id: item.call_id, content: item.output };
  });
}
function json(response: ServerResponse, body: unknown): void { response.writeHead(200, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store' }); response.end(stringifyWireJson(body)); }
function sseHeaders(response: ServerResponse): void { response.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-cache, no-store', Connection: 'keep-alive', 'X-Accel-Buffering': 'no' }); }
function sse(response: ServerResponse, data: unknown, event?: string): void { response.write(`${event ? `event: ${event}\n` : ''}data: ${stringifyWireJson(data)}\n\n`); }
