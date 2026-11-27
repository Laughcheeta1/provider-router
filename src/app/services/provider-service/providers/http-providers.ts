/** Stateless HTTP inference adapters; application tools are never executed here. */
import { z } from 'zod';
import { resolveWslCredentialHome } from './wsl-codex.ts';
import { CodexAuth, ExistingCodexCredentials, codexHome } from './codex-auth.ts';
import { MAX_PROVIDER_BYTES, providerOutputLimit, readProviderBody } from './provider-body.ts';
import { decodeCodexCatalog, decodeOpenRouterCatalog } from './provider-catalog.ts';
import { copyNumberSources, stringifyWireJson } from '../../../common/schemas/json-wire.ts';
import { nativeUnsignedField, parseNativeJson, replayArgumentsEqual } from '../../../common/schemas/native-json.ts';
import { tokenCount, tokenCountSchema } from '../../../common/schemas/token-count.ts';
import { nativeStruct, nativeStructObject } from '../../../common/schemas/native-struct.ts';
import { completionToolCallSchema, providerStateValueSchema, RouterError, type Connection, type Continuation, type InferenceRequest, type InferenceResult, type Message, type Model, type ProviderAdapter, type ToolCall } from '../../../common/schemas/contracts.ts';

const OPENROUTER_URL: string = 'https://openrouter.ai/api/v1';
const CODEX_URL: string = 'https://chatgpt.com/backend-api/codex';
const SSE_DATA_PREFIX: Buffer = Buffer.from('data:');
const SSE_NEWLINE: Buffer = Buffer.from('\n');
const SSE_DONE: Buffer = Buffer.from('[DONE]');
const chatMessage = z.object({ content: z.string().nullish(), tool_calls: z.array(providerStateValueSchema).nullish(), reasoning: z.unknown().optional(), reasoning_details: z.unknown().optional() });
const chatDecoder = nativeStruct(z.object({
  choices: z.array(nativeStruct(z.object({ finish_reason: z.string().nullish(), message: nativeStruct(chatMessage) }))).min(1),
  usage: nativeStruct(z.object({ prompt_tokens: tokenCountSchema, completion_tokens: tokenCountSchema, prompt_tokens_details: z.unknown().optional() }), undefined, undefined, ['prompt_tokens', 'completion_tokens']),
  provider: z.string().nullish(), model: z.string().nullish(), openrouter_metadata: z.unknown().optional(),
}));
type OpenRouterCompletion = z.infer<typeof chatDecoder>;
type OpenRouterState = Extract<Continuation['state'], { format: 'openrouter_chat' }>;
const providerToolCalls = z.array(completionToolCallSchema);

/** Normalize OpenRouter's published capabilities instead of inferring efforts from model names. */
export class OpenRouterAdapter implements ProviderAdapter {
  constructor(private readonly secret: (id: string) => string | undefined, private readonly fetcher: typeof fetch = fetch, private readonly base: string = OPENROUTER_URL) {}

  async infer(request: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult> {
    const headers: Record<string, string> = this.headers(connection);
    const { messages, route } = openRouterHistory(request.messages);
    const model: string = route?.resolved_model ?? request.model;
    const payload: Record<string, unknown> = {
      model, messages, stream: false,
      ...(request.tools.length ? { tools: request.tools } : {}),
      ...(request.response_format ? { response_format: request.response_format } : {}),
      ...(request.reasoning_effort !== undefined ? { reasoning: { effort: request.reasoning_effort } } : {}),
      ...(request.max_output_tokens ? { max_tokens: request.max_output_tokens } : {}),
      provider: { require_parameters: true, ...(route ? { only: [route.upstream], allow_fallbacks: false } : {}) },
      ...(request.session_id ? { session_id: request.session_id } : {}),
      ...(model.startsWith('anthropic/') ? { cache_control: { type: 'ephemeral' } } : {}),
      ...(request.tool_choice ? { tool_choice: request.tool_choice } : {}),
      ...(request.parallel_tool_calls !== undefined ? { parallel_tool_calls: request.parallel_tool_calls } : {}),
    };
    const response: Response = await providerTransport(this.fetcher(`${this.base}/chat/completions`, { method: 'POST', redirect: 'error', headers: { ...headers, 'X-OpenRouter-Metadata': 'enabled' }, body: stringifyWireJson(payload), signal }), signal);
    requireSuccess(response);
    const decoded: OpenRouterCompletion = await openRouterCompletion(response, signal);
    const usage: InferenceResult['usage'] = {
      input_tokens: decoded.usage.prompt_tokens, output_tokens: decoded.usage.completion_tokens,
      ...cacheUsage(decoded.usage.prompt_tokens_details),
    };
    const choice: OpenRouterCompletion['choices'][number] = decoded.choices[0]!;
    if (choice.finish_reason !== 'stop' && choice.finish_reason !== 'tool_calls') throw incomplete();
    const { reasoning_details, reasoning, content, tool_calls } = choice.message;
    let continuation: Continuation | undefined;
    if (decoded.model != null && (reasoning_details != null || reasoning != null)) {
      const upstream: string | undefined = route?.upstream ?? await this.endpointTag(decoded, headers, signal);
      if (upstream !== undefined) {
        const message: Record<string, unknown> = {
          role: 'assistant', content: content ?? null,
          ...(tool_calls != null ? { tool_calls } : {}),
          ...(reasoning_details != null ? { reasoning_details } : { reasoning }),
        };
        copyNumberSources(message, choice.message);
        continuation = { connection_id: connection.id, model: request.model, state: { format: 'openrouter_chat', message, upstream, resolved_model: decoded.model } };
      }
    }
    const calls: z.ZodSafeParseResult<ToolCall[]> = providerToolCalls.safeParse(tool_calls ?? []);
    if (!calls.success) throw new RouterError('INVALID_COMPLETION', 'The provider returned an invalid inference response.', 502);
    return {
      request_id: request.request_id, content: content ?? '', tool_calls: calls.data, usage,
      provider: 'openrouter', method: 'openrouter_http', model: request.model,
      ...(request.reasoning_effort !== undefined ? { reasoning_effort: request.reasoning_effort } : {}),
      ...(continuation ? { continuation } : {}),
    };
  }

  private async endpointTag(completion: OpenRouterCompletion, headers: Record<string, string>, signal: AbortSignal): Promise<string | undefined> {
    const provider: string | undefined = completion.provider ?? selectedOpenRouterProvider(completion.openrouter_metadata);
    const model: string = completion.model ?? '';
    const separator: number = model.indexOf('/');
    if (provider === undefined || separator < 0) return undefined;
    // Response metadata names the provider, while the request allowlist requires its endpoint tag.
    const author: string = encodeURIComponent(model.slice(0, separator));
    const name: string = encodeURIComponent(model.slice(separator + 1));
    try {
      const response: Response = await this.fetcher(`${this.base}/models/${author}/${name}/endpoints`, { headers, redirect: 'error', signal: AbortSignal.any([signal, AbortSignal.timeout(3000)]) });
      requireSuccess(response);
      const catalog: unknown = await providerJson(response, signal);
      const endpoints: unknown = object(object(catalog)?.data)?.endpoints;
      if (!Array.isArray(endpoints)) return undefined;
      const tags: Set<string> = new Set();
      for (const entry of endpoints as unknown[]) {
        const endpoint: Record<string, unknown> | undefined = object(entry);
        if (typeof endpoint?.tag === 'string' && (endpoint.provider_name === provider || endpoint.tag === provider)) tags.add(endpoint.tag);
      }
      return tags.size === 1 ? tags.values().next().value : undefined;
    } catch {
      signal.throwIfAborted();
      return undefined;
    }
  }

  async models(connection: Connection, signal: AbortSignal): Promise<Model[]> {
    const response: Response = await providerTransport(this.fetcher(`${this.base}/models`, { headers: this.headers(connection), redirect: 'error', signal }), signal);
    requireSuccess(response);
    return decodeOpenRouterCatalog(await providerBytes(response, signal));
  }

  private headers(connection: Connection): Record<string, string> {
    const key: string | undefined = this.secret(connection.id);
    if (!key) throw new RouterError('OPENROUTER_KEY_REQUIRED', 'Add an OpenRouter key to this connection in Provider Router.', 401);
    return { Authorization: `Bearer ${key}`, 'Content-Type': 'application/json' };
  }
}

/** Direct subscription HTTP uses existing Codex credentials, never platform API keys. */
export class CodexHttpAdapter implements ProviderAdapter {
  private readonly accounts: Map<string, CodexAuth> = new Map();
  constructor(private readonly fetcher: typeof fetch = fetch, private readonly base: string = CODEX_URL, private readonly authFactory: (home: string) => CodexAuth = (home: string): CodexAuth => new CodexAuth(home, new ExistingCodexCredentials(home))) {}

  async infer(request: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult> {
    const input: Record<string, unknown>[] = [];
    for (const message of request.messages) {
      if (message.continuation) {
        for (const item of codexHistory(message, message.continuation.state)) input.push(item);
        continue;
      }
      if (message.role === 'tool') {
        if (!message.tool_call_id) throw new RouterError('INVALID_HISTORY', 'Tool results must identify their tool call.');
        input.push({ type: 'function_call_output', call_id: message.tool_call_id, output: textContent(message.content) });
        continue;
      }
      const type: string = message.role === 'assistant' ? 'output_text' : 'input_text';
      const text: string = textContent(message.content);
      const content: Record<string, unknown>[] = Array.isArray(message.content)
        ? message.content.map((part): Record<string, unknown> => part.type === 'image_url'
          ? { type: 'input_image', image_url: (part.image_url as { url: string }).url }
          : { type, text: part.text })
        : text ? [{ type, text }] : [];
      if (content.length) input.push({ role: message.role === 'system' ? 'developer' : message.role, content });
      for (const call of message.tool_calls || []) input.push({ type: 'function_call', call_id: call.id, name: call.function.name, arguments: call.function.arguments });
    }
    const payload: Record<string, unknown> = {
      model: request.model, instructions: '', input, store: false, stream: true,
      tools: request.tools.map((tool) => ({ type: 'function', ...tool.function })),
      include: ['reasoning.encrypted_content'],
      ...(request.session_id ? { prompt_cache_key: request.session_id } : {}),
      ...(request.reasoning_effort !== undefined ? { reasoning: { effort: request.reasoning_effort, summary: 'auto' } } : {}),
    };
    if (request.tool_choice) payload.tool_choice = typeof request.tool_choice === 'object' ? { type: 'function', name: request.tool_choice.function.name } : request.tool_choice;
    if (request.parallel_tool_calls !== undefined) payload.parallel_tool_calls = request.parallel_tool_calls;
    // The subscription Responses endpoint controls its output budget and rejects max_output_tokens.
    // Enforce caller/model limits after completion rather than silently changing providers.
    if (request.response_format) payload.text = { format: { type: 'json_schema', ...request.response_format.json_schema } };
    // Codex uses the same session identity for the Responses header and cache key.
    const headers: Record<string, string> = request.session_id ? { 'session-id': request.session_id } : {};
    const response: Response = await this.request(connection, '/responses', { method: 'POST', body: stringifyWireJson(payload), headers, signal });
    const completed: Record<string, unknown> = await completedResponse(response, signal);
    if (completed.status !== 'completed') throw incomplete();
    const output: z.infer<typeof providerStateValueSchema>[] = decodeProviderResponse(z.array(providerStateValueSchema), completed.output);
    const calls: ToolCall[] = [];
    const texts: string[] = [];
    for (const item of output) {
      const type: unknown = object(item)?.type;
      if (type === 'function_call') {
        const call: { call_id: string; name: string; arguments: string } = decodeProviderResponse(z.object({ call_id: z.string(), name: z.string(), arguments: z.string() }), item);
        calls.push({ id: call.call_id, type: 'function', function: { name: call.name, arguments: call.arguments } });
      } else if (type === 'message') {
        const message: { content: unknown[] } = decodeProviderResponse(z.object({ content: z.array(z.unknown()) }), item);
        for (const value of message.content) {
          const part: Record<string, unknown> | undefined = object(value);
          if (part?.type === 'refusal') throw new RouterError('PROVIDER_REFUSAL', 'The provider declined this request.', 422);
          if (part?.type !== 'output_text') continue;
          if (typeof part.text !== 'string') throw invalidProviderResponse();
          texts.push(part.text);
        }
      } else if (type !== 'reasoning') throw new RouterError('UNSUPPORTED_PROVIDER_OUTPUT', 'The subscription returned an unsupported output item.', 502);
    }
    return {
      request_id: request.request_id, content: texts.join(''), tool_calls: calls,
      usage: providerUsage(completed.usage, 'input_tokens', 'output_tokens', 'input_tokens_details'),
      provider: 'codex', method: 'codex_http', model: request.model,
      ...(request.reasoning_effort !== undefined ? { reasoning_effort: request.reasoning_effort } : {}),
      continuation: { connection_id: connection.id, model: request.model, state: { format: 'codex_responses', items: output } },
    };
  }

  async models(connection: Connection, signal: AbortSignal): Promise<Model[]> {
    const response: Response = await this.request(connection, '/models?client_version=0.167.0', { signal });
    return decodeCodexCatalog(await providerBytes(response, signal));
  }

  private async request(connection: Connection, route: string, init: RequestInit & { headers?: Record<string, string> }): Promise<Response> {
    const home: string = connection.environment === 'wsl'
      ? await resolveWslCredentialHome(connection.wsl_distribution!, connection.codex_home)
      : codexHome(connection.codex_home);
    let auth: CodexAuth | undefined = this.accounts.get(home);
    if (!auth) { auth = this.authFactory(home); this.accounts.set(home, auth); }
    for (const force of [false, true]) {
      init.signal?.throwIfAborted();
      const response: Response = await providerTransport(this.fetcher(`${this.base}${route}`, { ...init, redirect: 'error', headers: { ...await auth.headers(force), 'Content-Type': 'application/json', Accept: 'text/event-stream', 'OpenAI-Beta': 'responses=experimental', originator: 'provider_router', 'User-Agent': 'provider-router/0.1.0', ...init.headers } }), init.signal);
      if (response.status === 401 && !force) { await response.body?.cancel().catch((): void => {}); continue; }
      requireSuccess(response); return response;
    }
    throw new RouterError('CODEX_AUTH_EXPIRED', 'The existing Codex login could not authenticate this request.', 401);
  }
}

function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}

function providerUsage(value: unknown, input: string, output: string, detailsField: string): InferenceResult['usage'] {
  const inputTokens: bigint | undefined = nativeUnsignedField(value, input);
  const outputTokens: bigint | undefined = nativeUnsignedField(value, output);
  if (inputTokens === undefined || outputTokens === undefined) throw invalidProviderResponse();
  return {
    input_tokens: tokenCount(inputTokens), output_tokens: tokenCount(outputTokens),
    ...cacheUsage(object(value)?.[detailsField]),
  };
}

function cacheUsage(details: unknown): Partial<InferenceResult['usage']> {
  const cached: bigint | undefined = nativeUnsignedField(details, 'cached_tokens');
  const written: bigint | undefined = nativeUnsignedField(details, 'cache_write_tokens');
  return {
    ...(cached !== undefined ? { cached_input_tokens: tokenCount(cached) } : {}),
    ...(written !== undefined ? { cache_write_tokens: tokenCount(written) } : {}),
  };
}

function textContent(content: unknown): string {
  if (typeof content === 'string') return content;
  if (!Array.isArray(content)) return '';
  return (content as unknown[]).map((part: unknown): unknown => object(part)?.text).filter((text: unknown): text is string => typeof text === 'string').join('\n');
}

function invalidContinuation(): RouterError {
  return new RouterError('INVALID_HISTORY', 'The assistant continuation is incompatible with this request.');
}

function sameCall(call: ToolCall, id: unknown, name: unknown, args: unknown): boolean {
  return call.id === id && call.function.name === name && typeof args === 'string'
    && replayArgumentsEqual(call.function.arguments, args);
}

function codexHistory(message: Message, state: Continuation['state']): Record<string, unknown>[] {
  if (state.format !== 'codex_responses') throw invalidContinuation();
  const calls: ToolCall[] = message.tool_calls ?? [];
  const items: Record<string, unknown>[] = [];
  let callIndex: number = 0;
  for (const value of state.items) {
    const item: Record<string, unknown> | undefined = object(value);
    if (!item) throw invalidContinuation();
    if (item.type === 'reasoning') {
      if (typeof item.encrypted_content !== 'string') continue;
    } else if (item.type === 'function_call') {
      const call: ToolCall | undefined = calls[callIndex++];
      if (!call || !sameCall(call, item.call_id, item.name, item.arguments)) throw invalidContinuation();
    } else if (item.type !== 'message' || item.role !== 'assistant') throw invalidContinuation();
    items.push(item);
  }
  if (callIndex !== calls.length) throw invalidContinuation();
  return items;
}

function openRouterHistory(history: Message[]): { messages: Record<string, unknown>[]; route: OpenRouterState | undefined } {
  const messages: Record<string, unknown>[] = [];
  let route: OpenRouterState | undefined;
  for (const { continuation, ...message } of history) {
    if (!continuation) { messages.push(message); continue; }
    const state: Continuation['state'] = continuation.state;
    if (state.format !== 'openrouter_chat' || !state.upstream || !state.resolved_model
      || (route && (route.upstream !== state.upstream || route.resolved_model !== state.resolved_model))) throw invalidContinuation();
    const native: Record<string, unknown> = state.message;
    const rawCalls: unknown[] = Array.isArray(native.tool_calls) ? native.tool_calls : [];
    const calls: ToolCall[] = message.tool_calls ?? [];
    if (native.role !== 'assistant' || textContent(native.content) !== textContent(message.content)
      || rawCalls.length !== calls.length || !calls.every((call: ToolCall, index: number): boolean => {
        const raw: Record<string, unknown> | undefined = object(rawCalls[index]);
        const fn: Record<string, unknown> | undefined = object(raw?.function);
        return sameCall(call, raw?.id, fn?.name, fn?.arguments);
      })) throw invalidContinuation();
    route = state;
    messages.push(native);
  }
  return { messages, route };
}

function selectedOpenRouterProvider(metadata: unknown): string | undefined {
  const available: unknown = object(object(metadata)?.endpoints)?.available;
  if (!Array.isArray(available)) return undefined;
  const selected: unknown = available.find((entry: unknown): boolean => object(entry)?.selected === true);
  const provider: unknown = object(selected)?.provider;
  return typeof provider === 'string' ? provider : undefined;
}

function requireSuccess(response: Response): void {
  if (response.ok) return;
  void response.body?.cancel().catch((): void => {});
  throw statusFailure(response.status);
}

function statusFailure(status: number): RouterError {
  switch (status) {
    case 400:
    case 413:
    case 422:
      return new RouterError('PROVIDER_REQUEST_REJECTED', 'The provider rejected the request as invalid.', 400);
    case 401:
      return new RouterError('PROVIDER_AUTH_REQUIRED', 'The provider rejected the credential. Check the connection in Provider Router.', 502);
    case 429:
      return new RouterError('PROVIDER_RATE_LIMIT', 'The provider is currently rate limited.', 429);
    default:
      return unavailable();
  }
}

function unavailable(): RouterError {
  return new RouterError('PROVIDER_UNAVAILABLE', 'The provider request failed. Check the connection in Provider Router.', 502);
}

function incomplete(): RouterError {
  return new RouterError('INCOMPLETE_COMPLETION', 'The provider did not finish the response.', 502);
}

function invalidProviderResponse(): RouterError {
  return new RouterError('PROVIDER_RESPONSE_INVALID', 'The provider returned an invalid response.', 502);
}

async function providerTransport<T>(operation: Promise<T>, signal?: AbortSignal | null): Promise<T> {
  try {
    return await operation;
  } catch (error: unknown) {
    signal?.throwIfAborted();
    if (error instanceof RouterError) throw error;
    throw unavailable();
  }
}

function parseProviderJson(bytes: Uint8Array): unknown {
  try {
    return parseNativeJson(bytes);
  } catch { throw invalidProviderResponse(); }
}

function decodeProviderResponse<T>(schema: z.ZodType<T>, value: unknown): T {
  const decoded: z.ZodSafeParseResult<T> = schema.safeParse(value);
  if (!decoded.success) throw invalidProviderResponse();
  return decoded.data;
}

async function openRouterCompletion(response: Response, signal: AbortSignal): Promise<OpenRouterCompletion> {
  const value: unknown = await providerJson(response, signal);
  const completion: Record<string, unknown> | undefined = object(value);
  const choices: unknown = completion?.choices;
  const firstChoice: unknown = Array.isArray(choices) ? choices[0] : undefined;
  // Inspect the selected error before requiring a complete choice or usage.
  const error: unknown = completion?.error ?? object(firstChoice)?.error;
  if (error != null) {
    const status: bigint | undefined = nativeUnsignedField(error, 'code');
    throw status !== undefined && status >= 400n && status <= 599n ? statusFailure(Number(status)) : incomplete();
  }
  const decoded: OpenRouterCompletion = decodeProviderResponse(chatDecoder, value);
  const rawCompletion: Record<string, unknown> | undefined = object(nativeStructObject(value, ['choices', 'usage', 'provider', 'model', 'openrouter_metadata']));
  const rawChoice: unknown = Array.isArray(rawCompletion?.choices) ? rawCompletion.choices[0] : undefined;
  const rawChoiceFields: Record<string, unknown> | undefined = object(nativeStructObject(rawChoice, ['finish_reason', 'message']));
  const rawMessage: Record<string, unknown> | undefined = object(nativeStructObject(rawChoiceFields?.message, Object.keys(chatMessage.shape)));
  if (rawMessage) copyNumberSources(decoded.choices[0]!.message, rawMessage);
  return decoded;
}

async function providerJson(response: Response, signal: AbortSignal): Promise<unknown> {
  return parseProviderJson(await providerBytes(response, signal));
}

function providerBytes(response: Response, signal: AbortSignal): Promise<Uint8Array> {
  return providerTransport(readProviderBody(response), signal);
}

function codexStreamFailure(error: unknown): RouterError {
  switch (object(error)?.code) {
    case 'server_error': return statusFailure(500);
    case 'rate_limit_exceeded': return statusFailure(429);
    default: return incomplete();
  }
}

/** SSE is parsed incrementally and bounded; incomplete streams never become successful answers. */
async function completedResponse(response: Response, signal: AbortSignal): Promise<Record<string, unknown>> {
  if (!response.body) throw incomplete();
  const reader: ReadableStreamDefaultReader<Uint8Array> = response.body.getReader();
  const completedItems: Map<bigint, Record<string, unknown>> = new Map();
  const lineParts: Buffer[] = [];
  const dataParts: Buffer[] = [];
  let received: number = 0;
  try {
    while (true) {
      signal.throwIfAborted();
      const frame: ReadableStreamReadResult<Uint8Array> = await providerTransport(reader.read(), signal);
      if (frame.done) break;
      received += frame.value.length;
      if (received > MAX_PROVIDER_BYTES) throw providerOutputLimit();
      // Keep comments and incomplete lines in bytes so only complete data events are decoded as JSON.
      const chunk: Buffer = Buffer.from(frame.value.buffer, frame.value.byteOffset, frame.value.byteLength);
      for (let start: number = 0; start < chunk.length;) {
        const end: number = chunk.indexOf(10, start);
        if (end === -1) { lineParts.push(chunk.subarray(start)); break; }
        const segment: Buffer = chunk.subarray(start, end);
        let line: Buffer = lineParts.length ? Buffer.concat([...lineParts, segment]) : segment;
        lineParts.length = 0;
        start = end + 1;
        if (line.at(-1) === 13) line = line.subarray(0, -1);
        if (line.length) {
          if (line.subarray(0, SSE_DATA_PREFIX.length).equals(SSE_DATA_PREFIX)) {
            if (dataParts.length) dataParts.push(SSE_NEWLINE);
            const part: Buffer = line.subarray(line[SSE_DATA_PREFIX.length] === 32 ? SSE_DATA_PREFIX.length + 1 : SSE_DATA_PREFIX.length);
            if (part.length) dataParts.push(part);
          }
          continue;
        }

        if (!dataParts.length) continue;
        const data: Buffer = Buffer.concat(dataParts);
        dataParts.length = 0;
        if (data.equals(SSE_DONE)) continue;
        const event: Record<string, unknown> | undefined = object(parseProviderJson(data));
        if (event?.type === 'response.failed') throw codexStreamFailure(object(event.response)?.error);
        if (event?.type === 'error') throw codexStreamFailure(event);
        if (event?.type === 'response.incomplete') throw incomplete();
        if (event?.type === 'response.completed') {
          const completed: Record<string, unknown> | undefined = object(event.response);
          if (completed) return mergeCodexItems(completed, completedItems);
        }
        if (event?.type === 'response.output_item.done') {
          const item: Record<string, unknown> | undefined = object(event.item);
          if (item) completedItems.set(nativeUnsignedField(event, 'output_index') ?? BigInt(completedItems.size), item);
        }
      }
    }
    throw incomplete();
  } finally { await reader.cancel().catch((): void => {}); reader.releaseLock(); }
}

function mergeCodexItems(response: Record<string, unknown>, completed: Map<bigint, Record<string, unknown>>): Record<string, unknown> {
  const terminal: unknown[] = Array.isArray(response.output) ? response.output : [];
  if (!completed.size || completed.size < terminal.length) return response;
  const items: Record<string, unknown>[] = [];
  // Prefer indexed completion events only when they contain a contiguous output sequence.
  for (let index: number = 0; index < completed.size; index++) {
    const item: Record<string, unknown> | undefined = completed.get(BigInt(index));
    if (!item) return response;
    items.push(item);
  }
  for (const value of terminal) {
    const item: Record<string, unknown> | undefined = object(value);
    if (typeof item?.id !== 'string') continue;
    const index: number = items.findIndex((entry: Record<string, unknown>): boolean => entry.id === item.id);
    if (index >= 0) {
      const current: Record<string, unknown> = items[index]!;
      const merged: Record<string, unknown> = { ...current, ...item };
      copyNumberSources(merged, current);
      copyNumberSources(merged, item);
      items[index] = merged;
    }
  }
  return { ...response, output: items };
}
