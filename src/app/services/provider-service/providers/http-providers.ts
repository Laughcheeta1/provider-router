/** Stateless HTTP inference adapters; application tools are never executed here. */
import { z } from 'zod';
import { resolveWslCredentialHome } from './wsl-codex.ts';
import { CodexAuth, ExistingCodexCredentials, codexHome } from './codex-auth.ts';
import { codexCatalog } from './codex-cli.ts';
import { RouterError, toolCallSchema, type Connection, type InferenceRequest, type InferenceResult, type Message, type Model, type ProviderAdapter, type ToolCall } from '../../../common/schemas/contracts.ts';

const OPENROUTER_URL: string = 'https://openrouter.ai/api/v1';
const CODEX_URL: string = 'https://chatgpt.com/backend-api/codex';
const number: z.ZodNumber = z.number().int().nonnegative();
const chatDecoder = z.object({ choices: z.array(z.object({ finish_reason: z.string().nullable(), message: z.object({ content: z.string().nullable().optional(), tool_calls: z.array(toolCallSchema).optional() }) })).min(1), usage: z.object({ prompt_tokens: number, completion_tokens: number }) });

/** Normalize OpenRouter's published capabilities instead of inferring efforts from model names. */
export class OpenRouterAdapter implements ProviderAdapter {
  constructor(private readonly secret: (id: string) => string | undefined, private readonly fetcher: typeof fetch = fetch, private readonly base: string = OPENROUTER_URL) {}

  async infer(request: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult> {
    const payload: Record<string, unknown> = {
      model: request.model, messages: request.messages, stream: false,
      ...(request.tools.length ? { tools: request.tools } : {}),
      ...(request.response_format ? { response_format: request.response_format } : {}),
      ...(request.reasoning_effort ? { reasoning: { effort: request.reasoning_effort } } : {}),
      ...(request.max_output_tokens ? { max_tokens: request.max_output_tokens } : {}),
      provider: { require_parameters: true },
      ...(request.tool_choice ? { tool_choice: request.tool_choice } : {}),
      ...(request.parallel_tool_calls !== undefined ? { parallel_tool_calls: request.parallel_tool_calls } : {}),
    };
    const response: Response = await this.fetcher(`${this.base}/chat/completions`, { method: 'POST', redirect: 'error', headers: this.headers(connection), body: JSON.stringify(payload), signal });
    requireSuccess(response);
    const decoded = chatDecoder.parse(await response.json());
    const choice = decoded.choices[0]!;
    if (choice.finish_reason !== 'stop' && choice.finish_reason !== 'tool_calls') throw new RouterError('INCOMPLETE_COMPLETION', 'The provider did not finish the response.', 502);
    return { request_id: request.request_id, content: choice.message.content || '', tool_calls: choice.message.tool_calls || [], usage: { input_tokens: decoded.usage.prompt_tokens, output_tokens: decoded.usage.completion_tokens }, provider: 'openrouter', method: 'openrouter_http', model: request.model, ...(request.reasoning_effort ? { reasoning_effort: request.reasoning_effort } : {}) };
  }

  async models(connection: Connection, signal: AbortSignal): Promise<Model[]> {
    const response: Response = await this.fetcher(`${this.base}/models`, { headers: this.headers(connection), redirect: 'error', signal });
    requireSuccess(response);
    const catalog = z.object({ data: z.array(z.object({ id: z.string(), name: z.string(), context_length: z.number().int().positive(), top_provider: z.object({ max_completion_tokens: z.number().int().positive().nullable() }), supported_parameters: z.array(z.string()).default([]), reasoning: z.object({ supported_efforts: z.array(z.string()).nullable().optional(), default_effort: z.string().nullable().optional(), mandatory: z.boolean().optional() }).optional() })) }).parse(await response.json());
    return catalog.data.map((entry): Model => ({
      id: entry.id, name: entry.name, reasoning_efforts: (entry.reasoning?.supported_efforts === null ? ['max', 'xhigh', 'high', 'medium', 'low', 'minimal', 'none'] : entry.reasoning?.supported_efforts || []).filter((effort: string): boolean => !entry.reasoning?.mandatory || effort !== 'none'), default_reasoning_effort: entry.reasoning?.default_effort || null,
      context_window: entry.context_length, max_output_tokens: entry.top_provider.max_completion_tokens || Math.floor(entry.context_length / 5), tools: entry.supported_parameters.includes('tools'), structured_output: entry.supported_parameters.includes('structured_outputs'), input_modalities: ['text'],
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
      if (message.role === 'tool') {
        if (!message.tool_call_id) throw new RouterError('INVALID_HISTORY', 'Tool results must identify their tool call.');
        input.push({ type: 'function_call_output', call_id: message.tool_call_id, output: textContent(message) });
        continue;
      }
      const text: string = textContent(message);
      if (text) input.push({ role: message.role === 'system' ? 'developer' : message.role, content: [{ type: message.role === 'assistant' ? 'output_text' : 'input_text', text }] });
      for (const call of message.tool_calls || []) input.push({ type: 'function_call', call_id: call.id, name: call.function.name, arguments: call.function.arguments });
    }
    const payload: Record<string, unknown> = { model: request.model, instructions: '', input, store: false, stream: true, tools: request.tools.map((tool) => ({ type: 'function', ...tool.function })), ...(request.reasoning_effort ? { reasoning: { effort: request.reasoning_effort } } : {}) };
    if (request.tool_choice) payload.tool_choice = typeof request.tool_choice === 'object' ? { type: 'function', name: request.tool_choice.function.name } : request.tool_choice;
    if (request.parallel_tool_calls !== undefined) payload.parallel_tool_calls = request.parallel_tool_calls;
    // The subscription Responses endpoint controls its output budget and rejects max_output_tokens.
    // Enforce caller/model limits after completion rather than silently changing providers.
    if (request.response_format) payload.text = { format: { type: 'json_schema', ...request.response_format.json_schema } };
    const response: Response = await this.request(connection, '/responses', { method: 'POST', body: JSON.stringify(payload), signal });
    const completed: Record<string, unknown> = await completedResponse(response, signal);
    const decoded = z.object({ status: z.literal('completed'), output: z.array(z.record(z.string(), z.unknown())), usage: z.object({ input_tokens: number, output_tokens: number }) }).parse(completed);
    const calls: ToolCall[] = [];
    const texts: string[] = [];
    for (const item of decoded.output) {
      if (item.type === 'function_call') {
        const call = z.object({ call_id: z.string(), name: z.string(), arguments: z.string() }).parse(item);
        calls.push({ id: call.call_id, type: 'function', function: { name: call.name, arguments: call.arguments } });
      } else if (item.type === 'message') {
        const message = z.object({ content: z.array(z.object({ type: z.string(), text: z.string().optional() })) }).parse(item);
        for (const part of message.content) { if (part.type === 'output_text' && part.text) texts.push(part.text); if (part.type === 'refusal') throw new RouterError('PROVIDER_REFUSAL', 'The provider declined this request.', 422); }
      } else if (item.type !== 'reasoning') throw new RouterError('UNSUPPORTED_PROVIDER_OUTPUT', 'The subscription returned an unsupported output item.', 502);
    }
    return { request_id: request.request_id, content: texts.join(''), tool_calls: calls, usage: decoded.usage, provider: 'codex', method: 'codex_http', model: request.model, ...(request.reasoning_effort ? { reasoning_effort: request.reasoning_effort } : {}) };
  }

  async models(connection: Connection, signal: AbortSignal): Promise<Model[]> {
    const response: Response = await this.request(connection, '/models?client_version=0.167.0', { signal });
    return codexCatalog(await response.json());
  }

  private async request(connection: Connection, route: string, init: RequestInit): Promise<Response> {
    const home: string = connection.environment === 'wsl'
      ? await resolveWslCredentialHome(connection.wsl_distribution!, connection.codex_home)
      : codexHome(connection.codex_home);
    let auth: CodexAuth | undefined = this.accounts.get(home);
    if (!auth) { auth = this.authFactory(home); this.accounts.set(home, auth); }
    for (const force of [false, true]) {
      init.signal?.throwIfAborted();
      const response: Response = await this.fetcher(`${this.base}${route}`, { ...init, redirect: 'error', headers: { ...await auth.headers(force), 'Content-Type': 'application/json', Accept: 'text/event-stream', 'OpenAI-Beta': 'responses=experimental', originator: 'provider_router', 'User-Agent': 'provider-router/0.1.0' } });
      if (response.status === 401 && !force) { await response.body?.cancel(); continue; }
      requireSuccess(response); return response;
    }
    throw new RouterError('CODEX_AUTH_EXPIRED', 'The existing Codex login could not authenticate this request.', 401);
  }
}

function textContent(message: Message): string {
  if (typeof message.content === 'string') return message.content;
  return (message.content || []).map((part): string => typeof part.text === 'string' ? part.text : '').join('\n');
}

function requireSuccess(response: Response): void {
  if (!response.ok) { void response.body?.cancel().catch((): void => {}); throw new RouterError(response.status === 429 ? 'PROVIDER_RATE_LIMIT' : response.status === 401 ? 'PROVIDER_AUTH_REQUIRED' : 'PROVIDER_UNAVAILABLE', response.status === 429 ? 'The provider is currently rate limited.' : 'The provider request failed. Check the connection in Provider Router.', response.status === 429 ? 429 : 502); }
}

/** SSE is parsed incrementally and bounded; incomplete streams never become successful answers. */
async function completedResponse(response: Response, signal: AbortSignal): Promise<Record<string, unknown>> {
  if (!response.body) throw new RouterError('INCOMPLETE_COMPLETION', 'The subscription returned no stream.', 502);
  const reader: ReadableStreamDefaultReader<Uint8Array> = response.body.getReader();
  const decoder: TextDecoder = new TextDecoder();
  let buffer: string = '';
  let received: number = 0;
  try {
    while (true) {
      signal.throwIfAborted();
      const frame: ReadableStreamReadResult<Uint8Array> = await reader.read();
      if (frame.done) break;
      received += frame.value.length;
      if (received > 16_777_216) throw new RouterError('PROVIDER_OUTPUT_LIMIT', 'The provider response exceeded the supported size.', 502);
      buffer += decoder.decode(frame.value, { stream: true });
      let separator: RegExpExecArray | null;
      while ((separator = /\r?\n\r?\n/.exec(buffer))) {
        const block: string = buffer.slice(0, separator.index); buffer = buffer.slice(separator.index + separator[0].length);
        const data: string = block.split(/\r?\n/).filter((line: string): boolean => line.startsWith('data:')).map((line: string): string => line.slice(5).trimStart()).join('\n');
        if (!data || data === '[DONE]') continue;
        const event = z.object({ type: z.string(), response: z.record(z.string(), z.unknown()).optional() }).parse(JSON.parse(data));
        if (event.type === 'response.completed' && event.response) return event.response;
        if (['response.failed', 'response.incomplete', 'error'].includes(event.type)) throw new RouterError('INCOMPLETE_COMPLETION', 'The provider did not complete the inference.', 502);
      }
    }
    throw new RouterError('INCOMPLETE_COMPLETION', 'The provider stream ended before completion.', 502);
  } finally { await reader.cancel().catch((): void => {}); reader.releaseLock(); }
}
