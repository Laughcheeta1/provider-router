/** Stateless HTTP inference adapters; application tools are never executed here. */
import { isDeepStrictEqual } from 'node:util';
import { z } from 'zod';
import { resolveWslCredentialHome } from './wsl-codex.ts';
import { CodexAuth, ExistingCodexCredentials, codexHome } from './codex-auth.ts';
import { codexCatalog } from './codex-cli.ts';
import { jsonObjectSchema, RouterError, toolCallSchema, type Connection, type Continuation, type InferenceRequest, type InferenceResult, type Message, type Model, type ProviderAdapter, type ToolCall } from '../../../common/schemas/contracts.ts';

const OPENROUTER_URL: string = 'https://openrouter.ai/api/v1';
const CODEX_URL: string = 'https://chatgpt.com/backend-api/codex';
const MAX_PROVIDER_BYTES: number = 16_777_216;
const number: z.ZodNumber = z.number().int().nonnegative();
const chatDecoder = z.object({
  choices: z.array(z.object({ finish_reason: z.string().nullish(), message: z.object({ content: z.string().nullish(), tool_calls: z.array(toolCallSchema).nullish(), reasoning_details: z.unknown().optional(), reasoning: z.unknown().optional() }) })).min(1),
  usage: z.object({ prompt_tokens: number, completion_tokens: number, prompt_tokens_details: z.unknown().optional() }),
  provider: z.string().nullish(), model: z.string().nullish(), openrouter_metadata: z.unknown().optional(),
});
type OpenRouterCompletion = z.infer<typeof chatDecoder>;
type OpenRouterState = Extract<Continuation['state'], { format: 'openrouter_chat' }>;
const cacheDetailsSchema = z.object({ cached_tokens: number.optional().catch(undefined), cache_write_tokens: number.optional().catch(undefined) });
const codexOutputSchema = z.object({ status: z.literal('completed'), output: z.array(z.json()), usage: z.object({ input_tokens: number, output_tokens: number, input_tokens_details: z.unknown().optional() }) });

/** Normalize OpenRouter's published capabilities instead of inferring efforts from model names. */
export class OpenRouterAdapter implements ProviderAdapter {
  constructor(private readonly secret: (id: string) => string | undefined, private readonly fetcher: typeof fetch = fetch, private readonly base: string = OPENROUTER_URL) {}

  async infer(request: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult> {
    const { messages, route } = openRouterHistory(request.messages);
    const model: string = route?.resolved_model ?? request.model;
    const payload: Record<string, unknown> = {
      model, messages, stream: false,
      ...(request.tools.length ? { tools: request.tools } : {}),
      ...(request.response_format ? { response_format: request.response_format } : {}),
      ...(request.reasoning_effort ? { reasoning: { effort: request.reasoning_effort } } : {}),
      ...(request.max_output_tokens ? { max_tokens: request.max_output_tokens } : {}),
      provider: { require_parameters: true, ...(route ? { only: [route.upstream], allow_fallbacks: false } : {}) },
      ...(request.session_id ? { session_id: request.session_id } : {}),
      ...(model.startsWith('anthropic/') ? { cache_control: { type: 'ephemeral' } } : {}),
      ...(request.tool_choice ? { tool_choice: request.tool_choice } : {}),
      ...(request.parallel_tool_calls !== undefined ? { parallel_tool_calls: request.parallel_tool_calls } : {}),
    };
    const headers: Record<string, string> = this.headers(connection);
    const response: Response = await this.fetcher(`${this.base}/chat/completions`, { method: 'POST', redirect: 'error', headers: { ...headers, 'X-OpenRouter-Metadata': 'enabled' }, body: JSON.stringify(payload), signal });
    requireSuccess(response);
    const decoded: OpenRouterCompletion = chatDecoder.parse(await providerJson(response));
    const choice: OpenRouterCompletion['choices'][number] = decoded.choices[0]!;
    if (choice.finish_reason !== 'stop' && choice.finish_reason !== 'tool_calls') throw new RouterError('INCOMPLETE_COMPLETION', 'The provider did not finish the response.', 502);
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
        continuation = { connection_id: connection.id, model: request.model, state: { format: 'openrouter_chat', message, upstream, resolved_model: decoded.model } };
      }
    }
    return {
      request_id: request.request_id, content: content ?? '', tool_calls: tool_calls ?? [],
      usage: { input_tokens: decoded.usage.prompt_tokens, output_tokens: decoded.usage.completion_tokens, ...cacheUsage(decoded.usage.prompt_tokens_details) },
      provider: 'openrouter', method: 'openrouter_http', model: request.model,
      ...(request.reasoning_effort ? { reasoning_effort: request.reasoning_effort } : {}),
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
      const catalog: unknown = await providerJson(response);
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
    const response: Response = await this.fetcher(`${this.base}/models`, { headers: this.headers(connection), redirect: 'error', signal });
    requireSuccess(response);
    const catalog = z.object({ data: z.array(z.object({ architecture: z.object({ input_modalities: z.array(z.string()) }).optional(), id: z.string(), name: z.string(), context_length: z.number().int().positive(), top_provider: z.object({ max_completion_tokens: z.number().int().positive().nullable() }), supported_parameters: z.array(z.string()).default([]), reasoning: z.object({ supported_efforts: z.array(z.string()).nullable().optional(), default_effort: z.string().nullable().optional(), mandatory: z.boolean().optional() }).optional() })) }).parse(await response.json());
    return catalog.data.map((entry): Model => ({
      id: entry.id, name: entry.name, reasoning_efforts: (entry.reasoning?.supported_efforts === null ? ['max', 'xhigh', 'high', 'medium', 'low', 'minimal', 'none'] : entry.reasoning?.supported_efforts || []).filter((effort: string): boolean => !entry.reasoning?.mandatory || effort !== 'none'), default_reasoning_effort: entry.reasoning?.default_effort || null,
      context_window: entry.context_length, max_output_tokens: entry.top_provider.max_completion_tokens || Math.floor(entry.context_length / 5), tools: entry.supported_parameters.includes('tools'), structured_output: entry.supported_parameters.includes('structured_outputs'), input_modalities: entry.architecture?.input_modalities.includes('image') ? ['text', 'image'] : ['text'],
    }));
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
      const content: Record<string, unknown>[] = Array.isArray(message.content) && message.content.some((part): boolean => part.type === 'image_url')
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
      ...(request.reasoning_effort ? { reasoning: { effort: request.reasoning_effort, summary: 'auto' } } : {}),
    };
    if (request.tool_choice) payload.tool_choice = typeof request.tool_choice === 'object' ? { type: 'function', name: request.tool_choice.function.name } : request.tool_choice;
    if (request.parallel_tool_calls !== undefined) payload.parallel_tool_calls = request.parallel_tool_calls;
    // The subscription Responses endpoint controls its output budget and rejects max_output_tokens.
    // Enforce caller/model limits after completion rather than silently changing providers.
    if (request.response_format) payload.text = { format: { type: 'json_schema', ...request.response_format.json_schema } };
    // Codex uses the same session identity for the Responses header and cache key.
    const headers: Record<string, string> = request.session_id ? { 'session-id': request.session_id } : {};
    const response: Response = await this.request(connection, '/responses', { method: 'POST', body: JSON.stringify(payload), headers, signal });
    const completed: Record<string, unknown> = await completedResponse(response, signal);
    const decoded: z.infer<typeof codexOutputSchema> = codexOutputSchema.parse(completed);
    const calls: ToolCall[] = [];
    const texts: string[] = [];
    for (const item of decoded.output) {
      const type: unknown = object(item)?.type;
      if (type === 'function_call') {
        const call = z.object({ call_id: z.string(), name: z.string(), arguments: z.string() }).parse(item);
        calls.push({ id: call.call_id, type: 'function', function: { name: call.name, arguments: call.arguments } });
      } else if (type === 'message') {
        const message = z.object({ content: z.array(z.object({ type: z.string(), text: z.string().optional() })) }).parse(item);
        for (const part of message.content) { if (part.type === 'output_text' && part.text) texts.push(part.text); if (part.type === 'refusal') throw new RouterError('PROVIDER_REFUSAL', 'The provider declined this request.', 422); }
      } else if (type !== 'reasoning') throw new RouterError('UNSUPPORTED_PROVIDER_OUTPUT', 'The subscription returned an unsupported output item.', 502);
    }
    return {
      request_id: request.request_id, content: texts.join(''), tool_calls: calls,
      usage: { input_tokens: decoded.usage.input_tokens, output_tokens: decoded.usage.output_tokens, ...cacheUsage(decoded.usage.input_tokens_details) },
      provider: 'codex', method: 'codex_http', model: request.model,
      ...(request.reasoning_effort ? { reasoning_effort: request.reasoning_effort } : {}),
      continuation: { connection_id: connection.id, model: request.model, state: { format: 'codex_responses', items: decoded.output } },
    };
  }

  async models(connection: Connection, signal: AbortSignal): Promise<Model[]> {
    const response: Response = await this.request(connection, '/models?client_version=0.167.0', { signal });
    return codexCatalog(await response.json());
  }

  private async request(connection: Connection, route: string, init: RequestInit & { headers?: Record<string, string> }): Promise<Response> {
    const home: string = connection.environment === 'wsl'
      ? await resolveWslCredentialHome(connection.wsl_distribution!, connection.codex_home)
      : codexHome(connection.codex_home);
    let auth: CodexAuth | undefined = this.accounts.get(home);
    if (!auth) { auth = this.authFactory(home); this.accounts.set(home, auth); }
    for (const force of [false, true]) {
      init.signal?.throwIfAborted();
      const response: Response = await this.fetcher(`${this.base}${route}`, { ...init, redirect: 'error', headers: { ...await auth.headers(force), 'Content-Type': 'application/json', Accept: 'text/event-stream', 'OpenAI-Beta': 'responses=experimental', originator: 'provider_router', 'User-Agent': 'provider-router/0.1.0', ...init.headers } });
      if (response.status === 401 && !force) { await response.body?.cancel(); continue; }
      requireSuccess(response); return response;
    }
    throw new RouterError('CODEX_AUTH_EXPIRED', 'The existing Codex login could not authenticate this request.', 401);
  }
}

function object(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}

function cacheUsage(details: unknown): Pick<InferenceResult['usage'], 'cached_input_tokens' | 'cache_write_tokens'> {
  const parsed: z.ZodSafeParseResult<z.infer<typeof cacheDetailsSchema>> = cacheDetailsSchema.safeParse(details);
  if (!parsed.success) return {};
  const { cached_tokens, cache_write_tokens } = parsed.data;
  return {
    ...(cached_tokens !== undefined ? { cached_input_tokens: cached_tokens } : {}),
    ...(cache_write_tokens !== undefined ? { cache_write_tokens } : {}),
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
  if (call.id !== id || call.function.name !== name || typeof args !== 'string') return false;
  try {
    const expected: unknown = JSON.parse(call.function.arguments);
    const actual: unknown = JSON.parse(args);
    return isDeepStrictEqual(expected, actual);
  } catch { return false; }
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
  if (!response.ok) { void response.body?.cancel().catch((): void => {}); throw new RouterError(response.status === 429 ? 'PROVIDER_RATE_LIMIT' : response.status === 401 ? 'PROVIDER_AUTH_REQUIRED' : 'PROVIDER_UNAVAILABLE', response.status === 429 ? 'The provider is currently rate limited.' : 'The provider request failed. Check the connection in Provider Router.', response.status === 429 ? 429 : 502); }
}

async function providerJson(response: Response): Promise<unknown> {
  let received: number = 0;
  const body: ReadableStream<Uint8Array> | undefined = response.body?.pipeThrough(new TransformStream({
    transform(chunk: Uint8Array, controller: TransformStreamDefaultController<Uint8Array>): void {
      received += chunk.byteLength;
      if (received > MAX_PROVIDER_BYTES) throw providerOutputLimit();
      controller.enqueue(chunk);
    },
  }));
  return new Response(body).json();
}

function providerOutputLimit(): RouterError {
  return new RouterError('PROVIDER_OUTPUT_LIMIT', 'The provider response exceeded the supported size.', 502);
}

const codexEventSchema = z.object({ type: z.string(), response: jsonObjectSchema.optional(), item: z.unknown().optional(), output_index: z.unknown().optional() });

/** SSE is parsed incrementally and bounded; incomplete streams never become successful answers. */
async function completedResponse(response: Response, signal: AbortSignal): Promise<Record<string, unknown>> {
  if (!response.body) throw new RouterError('INCOMPLETE_COMPLETION', 'The subscription returned no stream.', 502);
  const reader: ReadableStreamDefaultReader<Uint8Array> = response.body.getReader();
  const decoder: TextDecoder = new TextDecoder();
  const completedItems: Map<number, Record<string, unknown>> = new Map();
  let buffer: string = '';
  let received: number = 0;
  try {
    while (true) {
      signal.throwIfAborted();
      const frame: ReadableStreamReadResult<Uint8Array> = await reader.read();
      if (frame.done) break;
      received += frame.value.length;
      if (received > MAX_PROVIDER_BYTES) throw providerOutputLimit();
      buffer += decoder.decode(frame.value, { stream: true });
      let separator: RegExpExecArray | null;
      while ((separator = /\r?\n\r?\n/.exec(buffer))) {
        const block: string = buffer.slice(0, separator.index); buffer = buffer.slice(separator.index + separator[0].length);
        const data: string = block.split(/\r?\n/).filter((line: string): boolean => line.startsWith('data:')).map((line: string): string => line.slice(5).trimStart()).join('\n');
        if (!data || data === '[DONE]') continue;
        const event: z.infer<typeof codexEventSchema> = codexEventSchema.parse(JSON.parse(data));
        if (event.type === 'response.completed' && event.response) return mergeCodexItems(event.response, completedItems);
        if (event.type === 'response.output_item.done') {
          const item: Record<string, unknown> | undefined = object(event.item);
          const index: z.ZodSafeParseResult<number> = number.safeParse(event.output_index);
          if (item) completedItems.set(index.success ? index.data : completedItems.size, item);
        }
        if (['response.failed', 'response.incomplete', 'error'].includes(event.type)) throw new RouterError('INCOMPLETE_COMPLETION', 'The provider did not complete the inference.', 502);
      }
    }
    throw new RouterError('INCOMPLETE_COMPLETION', 'The provider stream ended before completion.', 502);
  } finally { await reader.cancel().catch((): void => {}); reader.releaseLock(); }
}

function mergeCodexItems(response: Record<string, unknown>, completed: Map<number, Record<string, unknown>>): Record<string, unknown> {
  const terminal: unknown[] = Array.isArray(response.output) ? response.output : [];
  if (!completed.size || completed.size < terminal.length) return response;
  const items: Record<string, unknown>[] = [];
  // Prefer indexed completion events only when they contain a contiguous output sequence.
  for (let index: number = 0; index < completed.size; index++) {
    const item: Record<string, unknown> | undefined = completed.get(index);
    if (!item) return response;
    items.push(item);
  }
  for (const value of terminal) {
    const item: Record<string, unknown> | undefined = object(value);
    if (typeof item?.id !== 'string') continue;
    const index: number = items.findIndex((entry: Record<string, unknown>): boolean => entry.id === item.id);
    if (index >= 0) items[index] = { ...items[index], ...item };
  }
  return { ...response, output: items };
}
