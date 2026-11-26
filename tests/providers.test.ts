/** Provider wire contracts and OAuth rotation use deterministic synthetic endpoints. */
import { afterEach, describe, expect, it } from 'vitest';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { CodexAuth, ExistingCodexCredentials, codexAuthSettings, type CodexCredentials, type CredentialStore } from '../src/app/services/provider-service/providers/codex-auth.ts';
import { CodexHttpAdapter, OpenRouterAdapter } from '../src/app/services/provider-service/providers/http-providers.ts';
import { CodexCliAdapter, codexCatalog } from '../src/app/services/provider-service/providers/codex-cli.ts';
import { uuid7 } from '../src/app/common/identity/identity.ts';
import type { Connection, Continuation, InferenceRequest, InferenceResult, ProviderAdapter, ToolCall } from '../src/app/common/schemas/contracts.ts';
const directories: string[] = [];
afterEach(async (): Promise<void> => { await Promise.all(directories.splice(0).map((directory: string): Promise<void> => rm(directory, { recursive: true, force: true }))); });
async function temporary(): Promise<string> { const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-provider-test-')); directories.push(directory); return directory; }
function connection(method: Connection['method']): Connection { return { id: uuid7(), name: 'Synthetic provider', method, exposed: true, models: [] }; }
function request(target: Connection): InferenceRequest { return { request_id: uuid7(), connection_id: target.id, model: 'synthetic-model', reasoning_effort: 'low', messages: [{ role: 'system', content: 'Synthetic standing instruction.' }, { role: 'user', content: 'Synthetic question.' }], tools: [] }; }
function credentials(expiration: number): CodexCredentials { return { auth_mode: 'chatgpt', last_refresh: new Date().toISOString(), tokens: { access_token: `header.${Buffer.from(JSON.stringify({ exp: expiration })).toString('base64url')}.signature`, refresh_token: 'synthetic-refresh', id_token: 'synthetic-identity', account_id: 'synthetic-account', login_owner_id: 'synthetic-login' } }; }
async function syntheticCodex(fetcher: typeof fetch): Promise<CodexHttpAdapter> {
  const home: string = await temporary();
  let stored: CodexCredentials = credentials(Date.now() / 1000 + 3600);
  const auth: CodexAuth = new CodexAuth(home, {
    load: async (): Promise<CodexCredentials> => stored,
    save: async (_expected, updated): Promise<CodexCredentials> => { stored = updated; return stored; },
  }, async (): Promise<Response> => Response.json({ access_token: 'rotated-synthetic-access' }), 'https://synthetic.example/oauth/token');
  return new CodexHttpAdapter(fetcher, 'https://synthetic.example', (): CodexAuth => auth);
}

const codexTextOutput: string = '[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Answer"}]}]';
function codexCompletion(usage: string, output: string = codexTextOutput): string {
  return `data: {"type":"response.completed","response":{"status":"completed","output":${output},"usage":${usage}}}\n\n`;
}

describe.each(['codex_http', 'openrouter_http'] as const)('%s completion and replay', (method): void => {
  const isCodex: boolean = method === 'codex_http';
  const inputField: string = isCodex ? 'input_tokens' : 'prompt_tokens';
  const outputField: string = isCodex ? 'output_tokens' : 'completion_tokens';
  const detailsField: string = isCodex ? 'input_tokens_details' : 'prompt_tokens_details';
  const ordinaryUsage: string = `{"${inputField}":11,"${outputField}":2}`;

  async function adapter(usage: string, onRequest?: (payload: unknown) => void): Promise<ProviderAdapter> {
    const fetcher: typeof fetch = async (_url, init): Promise<Response> => {
      onRequest?.(JSON.parse(String(init?.body)));
      return new Response(isCodex ? codexCompletion(usage) : `{"choices":[{"finish_reason":"stop","message":{"content":"Answer"}}],"usage":${usage}}`);
    };
    return isCodex ? syntheticCodex(fetcher) : new OpenRouterAdapter((): string => 'synthetic-key', fetcher);
  }

  it.each([
    ['large integer', '{"n":9007199254740992}', '{"n":9007199254740993}', false],
    ['integer and decimal', '{"n":1}', '{"n":1.0}', false],
    ['integer and exponent', '{"n":1}', '{"n":1e0}', false],
    ['signed integer boundary', '{"n":-9223372036854775808}', '{"n":-9223372036854775809}', false],
    ['unsigned integer boundary', '{"n":18446744073709551615}', '{"n":18446744073709551616}', false],
    ['object key order', '{"a":{"id":9007199254740993,"name":"x"},"b":[1.0]}', '{"b":[1.0],"a":{"name":"x","id":9007199254740993}}', true],
    ['float spellings', '{"n":1e0}', '{"n":1.0}', true],
    ['native float rounding', '{"n":9007199254740993.0}', '{"n":9007199254740992.0}', false],
    ['floating-point zero signs', '{"n":-0}', '{"n":0.0}', true],
    ['a duplicate key with an invalid number', '{"n":1e999,"n":1}', '{"n":1}', false],
    ['trailing invalid input', '{"n":1}x', '{"n":1}', false],
  ] as const)('compares replay arguments with %s using native JSON semantics', async (_label, returned, replayed, matches): Promise<void> => {
    const target: Connection = connection(method);
    const input: InferenceRequest = request(target);
    const call: ToolCall = { id: 'call-1', type: 'function', function: { name: 'lookup', arguments: returned } };
    const codexCall: Record<string, string> = { type: 'function_call', call_id: call.id, name: call.function.name, arguments: returned };
    const nativeMessage: Record<string, unknown> = { role: 'assistant', content: null, tool_calls: [call] };
    const state: Continuation['state'] = isCodex ? { format: 'codex_responses', items: [codexCall] }
      : { format: 'openrouter_chat', message: nativeMessage, upstream: 'synthetic', resolved_model: 'synthetic/resolved' };
    input.messages.push({ role: 'assistant', content: null, tool_calls: [{ ...call, function: { ...call.function, arguments: replayed } }], continuation: { connection_id: target.id, model: input.model, state } }, { role: 'tool', tool_call_id: call.id, content: '42' });
    const sent: unknown[] = [];
    const provider: ProviderAdapter = await adapter(ordinaryUsage, (payload: unknown): void => { sent.push(payload); });
    const result: Promise<InferenceResult> = provider.infer(input, target, AbortSignal.timeout(1000));
    if (matches) {
      await expect(result).resolves.toMatchObject({ content: 'Answer' });
      expect(sent).toEqual([expect.objectContaining(isCodex ? { input: expect.arrayContaining([codexCall]) }
        : { messages: expect.arrayContaining([nativeMessage]), model: 'synthetic/resolved', provider: { require_parameters: true, only: ['synthetic'], allow_fallbacks: false } })]);
    } else {
      await expect(result).rejects.toMatchObject({ status: 400, code: 'INVALID_HISTORY' });
      expect(sent).toEqual([]);
    }
  });

  it.each([
    ['7.0', '4', { cache_write_tokens: 4 }],
    ['0', '4e0', { cached_input_tokens: 0 }],
    ['-0', '18446744073709551616', {}],
  ] as const)('omits invalid cache counters independently (%s and %s)', async (cached, written, expected): Promise<void> => {
    const target: Connection = connection(method);
    const provider: ProviderAdapter = await adapter(`{"${inputField}":11,"${outputField}":2,"${detailsField}":{"cached_tokens":${cached},"cache_write_tokens":${written}}}`);
    const result: InferenceResult = await provider.infer(request(target), target, AbortSignal.timeout(1000));
    expect(result.usage).toEqual({ input_tokens: 11, output_tokens: 2, ...expected });
  });

  it.each([['11.0', '2'], ['11', '2e0'], ['-0', '2'], ['11', '18446744073709551616']] as const)(
    'rejects usage with input %s and output %s when either token is outside u64', async (input, output): Promise<void> => {
      const target: Connection = connection(method);
      const provider: ProviderAdapter = await adapter(`{"${inputField}":${input},"${outputField}":${output}}`);
      await expect(provider.infer(request(target), target, AbortSignal.timeout(1000))).rejects.toMatchObject({ status: 502, code: 'PROVIDER_RESPONSE_INVALID' });
    });
});
describe('Codex subscription credentials', (): void => {
  it('refreshes once for simultaneous requests and preserves identity plus returned rotated tokens', async (): Promise<void> => {
    let stored: CodexCredentials = credentials(1); let exchanges: number = 0;
    const storage: CredentialStore = { load: async (): Promise<CodexCredentials> => structuredClone(stored), save: async (_expected, updated): Promise<CodexCredentials> => { stored = updated; return updated; } };
    const auth: CodexAuth = new CodexAuth(await temporary(), storage, async (): Promise<Response> => { exchanges++; return Response.json({ access_token: credentials(Date.now() / 1000 + 3600).tokens.access_token, refresh_token: 'rotated-refresh' }); });
    const values: Record<string, string>[] = await Promise.all([auth.headers(), auth.headers()]);
    expect(exchanges).toBe(1); expect(values[0]).toEqual(values[1]); expect(stored.tokens.id_token).toBe('synthetic-identity'); expect(stored.tokens.account_id).toBe('synthetic-account'); expect(stored.tokens.refresh_token).toBe('rotated-refresh');
  });
  it('reloads externally rotated credentials after 401 instead of refreshing stale tokens', async (): Promise<void> => {
    let stored: CodexCredentials = credentials(Date.now() / 1000 + 3600); let exchanges: number = 0;
    const storage: CredentialStore = { load: async (): Promise<CodexCredentials> => stored, save: async (_expected, updated): Promise<CodexCredentials> => updated };
    const auth: CodexAuth = new CodexAuth(await temporary(), storage, async (): Promise<Response> => { exchanges++; throw new Error('Must not exchange'); });
    await auth.headers(); stored = { ...stored, tokens: { ...stored.tokens, access_token: 'newer-access' } };
    expect(await auth.headers(true)).toMatchObject({ Authorization: 'Bearer newer-access' }); expect(exchanges).toBe(0);
  });
  it('keeps issuer and client bound to persisted OAuth provenance', async (): Promise<void> => {
    const saved: CodexCredentials = { ...credentials(1), tokens: { ...credentials(1).tokens, oauth_provenance: { issuer: 'https://identity.example', client_id: 'original-client' } } };
    let target: string = ''; let body: string = '';
    const auth: CodexAuth = new CodexAuth(await temporary(), { load: async (): Promise<CodexCredentials> => saved, save: async (_expected, updated): Promise<CodexCredentials> => updated }, async (url, init): Promise<Response> => { target = String(url); body = String(init?.body); return Response.json({ access_token: 'rotated' }); });
    await auth.headers(); expect(target).toBe('https://identity.example/oauth/token'); expect(JSON.parse(body)).toMatchObject({ client_id: 'original-client' });
  });
  it('commits only the original login generation and never overwrites another account', async (): Promise<void> => {
    const home: string = await temporary(); const original: CodexCredentials = credentials(1); const file: string = path.join(home, 'auth.json');
    await writeFile(file, JSON.stringify(original)); const storage: ExistingCodexCredentials = new ExistingCodexCredentials(home);
    const other: CodexCredentials = { ...original, tokens: { ...original.tokens, account_id: 'different-account' } };
    await writeFile(file, JSON.stringify(other));
    await expect(storage.save(original, { ...original, tokens: { ...original.tokens, access_token: 'new' } })).rejects.toMatchObject({ code: 'CODEX_ACCOUNT_CHANGED' });
    expect(JSON.parse(await readFile(file, 'utf8'))).toEqual(other);
  });
  it('stops after an uncertain refresh instead of replaying a potentially consumed grant', async (): Promise<void> => {
    const home: string = await temporary(); let exchanges: number = 0; const saved: CodexCredentials = credentials(1);
    const storage: CredentialStore = { load: async (): Promise<CodexCredentials> => saved, save: async (_expected, updated): Promise<CodexCredentials> => updated };
    const auth: CodexAuth = new CodexAuth(home, storage, async (): Promise<Response> => { exchanges++; throw new Error('Synthetic connection lost after exchange'); });
    await expect(auth.headers()).rejects.toThrow('Synthetic connection lost');
    await expect(auth.headers()).rejects.toMatchObject({ code: 'CODEX_REFRESH_UNCERTAIN' });
    expect(exchanges).toBe(1);
    const marker: string = await readFile(path.join(home, 'auth.provider-router-refresh.json'), 'utf8'); expect(marker).not.toContain(saved.tokens.refresh_token); expect(marker).not.toContain(saved.tokens.access_token);
  });
  it('retains a rotated credential if inference is cancelled while refresh is committing', async (): Promise<void> => {
    const home: string = await temporary(); const original: CodexCredentials = credentials(1); let stored: CodexCredentials = original;
    let finish: (() => void) | undefined;
    const gate: Promise<void> = new Promise((resolve): void => { finish = resolve; });
    let announce: (() => void) | undefined; const committing: Promise<void> = new Promise((resolve): void => { announce = resolve; });
    const auth: CodexAuth = new CodexAuth(home, { load: async (): Promise<CodexCredentials> => stored, save: async (_expected, updated): Promise<CodexCredentials> => { announce!(); await gate; stored = updated; return updated; } }, async (): Promise<Response> => Response.json({ access_token: 'new-access', refresh_token: 'new-refresh' }));
    const adapter: CodexHttpAdapter = new CodexHttpAdapter(async (_url, init): Promise<Response> => { init?.signal?.throwIfAborted(); throw new Error('The cancelled inference must not reach the provider.'); }, 'https://synthetic.example', (): CodexAuth => auth);
    const controller: AbortController = new AbortController(); const target: Connection = connection('codex_http');
    const pending: Promise<InferenceResult> = adapter.infer(request(target), target, controller.signal);
    await committing; controller.abort(); finish!();
    await expect(pending).rejects.toThrow(); expect(stored.tokens.refresh_token).toBe('new-refresh');
    await expect(readFile(path.join(home, 'auth.provider-router-refresh.json'))).rejects.toThrow();
  });
  it('reads only root authentication settings using TOML semantics and rejects unsupported encrypted storage', async (): Promise<void> => {
    const home: string = await temporary();
    await writeFile(path.join(home, 'config.toml'), "cli_auth_credentials_store = 'keyring'\n[features]\nsecret_auth_storage = true\n[irrelevant]\ncli_auth_credentials_store = 'file'\n");
    expect(await codexAuthSettings(home)).toEqual({ store: 'keyring', secret_auth_storage: true });
    await expect(new ExistingCodexCredentials(home).load()).rejects.toMatchObject({ code: 'CODEX_STORAGE_UNSUPPORTED' });
  });
});
describe('HTTP inference adapters', (): void => {
  it.each([
    [400, 400, 'PROVIDER_REQUEST_REJECTED'],
    [413, 400, 'PROVIDER_REQUEST_REJECTED'],
    [422, 400, 'PROVIDER_REQUEST_REJECTED'],
    [401, 502, 'PROVIDER_AUTH_REQUIRED'],
    [403, 502, 'PROVIDER_UNAVAILABLE'],
    [404, 502, 'PROVIDER_UNAVAILABLE'],
    [429, 429, 'PROVIDER_RATE_LIMIT'],
    [500, 502, 'PROVIDER_UNAVAILABLE'],
    [503, 502, 'PROVIDER_UNAVAILABLE'],
  ] as const)('maps upstream HTTP %i in both inference adapters', async (upstream, status, code): Promise<void> => {
    const fetcher: typeof fetch = async (): Promise<Response> => new Response('upstream-private-detail', { status: upstream });
    const openRouter: Connection = connection('openrouter_http');
    const codex: Connection = connection('codex_http');
    const signal: AbortSignal = AbortSignal.timeout(1000);
    const expected: { status: number; code: string; message?: string } = { status, code };
    if (status === 400) expected.message = 'The provider rejected the request as invalid.';
    await expect(new OpenRouterAdapter((): string => 'synthetic-key', fetcher).infer(request(openRouter), openRouter, signal)).rejects.toMatchObject(expected);
    await expect((await syntheticCodex(fetcher)).infer(request(codex), codex, signal)).rejects.toMatchObject(expected);
  });

  it('maps synthetic transport errors to provider unavailability for both adapters', async (): Promise<void> => {
    const fetcher: typeof fetch = async (): Promise<never> => { throw new TypeError('Synthetic provider disconnected'); };
    const openRouter: Connection = connection('openrouter_http');
    const codex: Connection = connection('codex_http');
    const signal: AbortSignal = AbortSignal.timeout(1000);
    const expected: { status: number; code: string } = { status: 502, code: 'PROVIDER_UNAVAILABLE' };
    await expect(new OpenRouterAdapter((): string => 'synthetic-key', fetcher).infer(request(openRouter), openRouter, signal)).rejects.toMatchObject(expected);
    await expect((await syntheticCodex(fetcher)).infer(request(codex), codex, signal)).rejects.toMatchObject(expected);
  });

  it.each([
    ['{"code":502}', 502, 'PROVIDER_UNAVAILABLE'],
    ['{"code":402}', 502, 'PROVIDER_UNAVAILABLE'],
    ['{"code":429}', 429, 'PROVIDER_RATE_LIMIT'],
    ['{"code":400}', 400, 'PROVIDER_REQUEST_REJECTED'],
    ['{"code":401}', 502, 'PROVIDER_AUTH_REQUIRED'],
    ['{"code":301}', 502, 'INCOMPLETE_COMPLETION'],
    ['{"code":"429"}', 502, 'INCOMPLETE_COMPLETION'],
    ['{"code":429.0}', 502, 'INCOMPLETE_COMPLETION'],
    ['{"code":4.29e2}', 502, 'INCOMPLETE_COMPLETION'],
    ['{"message":"upstream-private-detail"}', 502, 'INCOMPLETE_COMPLETION'],
  ] as const)('classifies OpenRouter embedded error %s before decoding a completion', async (error, status, code): Promise<void> => {
    const target: Connection = connection('openrouter_http');
    const input: InferenceRequest = request(target);
    const bodies: string[] = [
      `{"error":${error}}`,
      `{"error":null,"choices":[{"error":${error},"finish_reason":"error","message":{"content":"partial"}}]}`,
      `{"choices":[{"error":${error},"finish_reason":"stop","message":{"content":"partial"}}],"usage":{"prompt_tokens":7,"completion_tokens":2}}`,
    ];
    for (const body of bodies) {
      const adapter: OpenRouterAdapter = new OpenRouterAdapter((): string => 'synthetic-key', async (): Promise<Response> => new Response(body));
      await expect(adapter.infer(input, target, AbortSignal.timeout(1000))).rejects.toMatchObject({ status, code });
    }
  });

  it('gives the top-level OpenRouter error precedence over the first choice', async (): Promise<void> => {
    const target: Connection = connection('openrouter_http');
    const adapter: OpenRouterAdapter = new OpenRouterAdapter((): string => 'synthetic-key', async (): Promise<Response> => Response.json({
      error: { message: 'upstream-private-detail' }, choices: [{ error: { code: 429 } }],
    }));
    await expect(adapter.infer(request(target), target, AbortSignal.timeout(1000))).rejects.toMatchObject({ status: 502, code: 'INCOMPLETE_COMPLETION' });
  });

  it('accepts a completed first OpenRouter choice with null errors and an error in a later choice', async (): Promise<void> => {
    const target: Connection = connection('openrouter_http');
    const adapter: OpenRouterAdapter = new OpenRouterAdapter((): string => 'synthetic-key', async (): Promise<Response> => Response.json({
      error: null, choices: [
        { error: null, finish_reason: 'stop', message: { content: 'done' } },
        { error: { code: 429 }, finish_reason: 'error', message: { content: 'partial' } },
      ], usage: { prompt_tokens: 7, completion_tokens: 2 },
    }));
    await expect(adapter.infer(request(target), target, AbortSignal.timeout(1000))).resolves.toMatchObject({ content: 'done' });
  });

  it.each([
    [{ type: 'response.failed', response: { error: { code: 'server_error' } } }, 502, 'PROVIDER_UNAVAILABLE'],
    [{ type: 'error', code: 'server_error' }, 502, 'PROVIDER_UNAVAILABLE'],
    [{ type: 'response.failed', response: { error: { code: 'rate_limit_exceeded' } } }, 429, 'PROVIDER_RATE_LIMIT'],
    [{ type: 'error', code: 'rate_limit_exceeded', response: null }, 429, 'PROVIDER_RATE_LIMIT'],
    [{ type: 'response.failed', response: { error: { code: 'invalid_prompt' } } }, 502, 'INCOMPLETE_COMPLETION'],
    [{ type: 'response.failed', response: null }, 502, 'INCOMPLETE_COMPLETION'],
    [{ type: 'response.incomplete' }, 502, 'INCOMPLETE_COMPLETION'],
    [{ type: 'response.completed', response: { status: 'incomplete' } }, 502, 'INCOMPLETE_COMPLETION'],
  ] as const)('preserves the upstream meaning of Codex SSE event %j', async (event, status, code): Promise<void> => {
    const target: Connection = connection('codex_http');
    const adapter: CodexHttpAdapter = await syntheticCodex(async (): Promise<Response> => new Response(`data: ${JSON.stringify(event)}\n\n`));
    await expect(adapter.infer(request(target), target, AbortSignal.timeout(1000))).rejects.toMatchObject({ status, code });
  });

  it('classifies a malformed completion as an invalid provider response in both adapters', async (): Promise<void> => {
    const openRouter: Connection = connection('openrouter_http');
    const codex: Connection = connection('codex_http');
    const signal: AbortSignal = AbortSignal.timeout(1000);
    const expected: { status: number; code: string } = { status: 502, code: 'PROVIDER_RESPONSE_INVALID' };
    const openRouterAdapter: OpenRouterAdapter = new OpenRouterAdapter((): string => 'synthetic-key', async (): Promise<Response> => Response.json({ choices: [] }));
    const codexAdapter: CodexHttpAdapter = await syntheticCodex(async (): Promise<Response> => new Response('data: {"type":"response.completed","response":{"status":"completed"}}\n\n'));
    await expect(openRouterAdapter.infer(request(openRouter), openRouter, signal)).rejects.toMatchObject(expected);
    await expect(codexAdapter.infer(request(codex), codex, signal)).rejects.toMatchObject(expected);
  });

  it('uses published reasoning options and keeps mandatory reasoning enabled in model discovery', async (): Promise<void> => {
    const target: Connection = connection('openrouter_http');
    const adapter: OpenRouterAdapter = new OpenRouterAdapter((): string => 'synthetic-api-key', async (): Promise<Response> => Response.json({ data: [{ id: 'mandatory-model', name: 'Mandatory', context_length: 10000, top_provider: { max_completion_tokens: null }, supported_parameters: ['tools', 'structured_outputs'], reasoning: { supported_efforts: null, mandatory: true, default_effort: 'high' } }, { id: 'ordinary-model', name: 'Ordinary', context_length: 10000, top_provider: { max_completion_tokens: 1000 }, supported_parameters: [] }] }));
    const models = await adapter.models(target, AbortSignal.timeout(1000));
    expect(models[0]?.reasoning_efforts).toContain('high'); expect(models[0]?.reasoning_efforts).not.toContain('none'); expect(models[0]?.max_output_tokens).toBe(2000); expect(models[1]?.reasoning_efforts).toEqual([]);
  });
  it('sends OpenRouter selected effort, complete tool associations and JSON schema without changing billing providers', async (): Promise<void> => {
    const target: Connection = connection('openrouter_http'); const input: InferenceRequest = { ...request(target), messages: [{ role: 'assistant', content: '', tool_calls: [{ id: 'call1', type: 'function', function: { name: 'read_balance', arguments: '{"id":"a"}' } }] }, { role: 'tool', tool_call_id: 'call1', content: '{"balance":42}' }], response_format: { type: 'json_schema', json_schema: { name: 'answer', schema: { type: 'object' } } } };
    let sent: Record<string, unknown> = {}; let headers: Record<string, string> = {};
    const adapter: OpenRouterAdapter = new OpenRouterAdapter((): string => 'synthetic-api-key', async (_url, init): Promise<Response> => { sent = JSON.parse(String(init?.body)); headers = init?.headers as Record<string, string>; return Response.json({ choices: [{ finish_reason: 'stop', message: { content: '{"balance":42}' } }], usage: { prompt_tokens: 11, completion_tokens: 8 } }); });
    const result: InferenceResult = await adapter.infer(input, target, AbortSignal.timeout(1000)); expect(result.content).toBe('{"balance":42}'); expect(sent).toMatchObject({ reasoning: { effort: 'low' }, messages: input.messages, response_format: input.response_format }); expect(headers.Authorization).toBe('Bearer synthetic-api-key');
  });
  it('resolves the OpenRouter route for assistant reasoning and pins it on replay', async (): Promise<void> => {
    const target: Connection = connection('openrouter_http');
    const input: InferenceRequest = { ...request(target), model: 'anthropic/synthetic', session_id: uuid7() };
    const resolved: string = 'anthropic/resolved';
    const message: Record<string, unknown> = { role: 'assistant', content: 'First answer', reasoning_details: [{ type: 'reasoning.encrypted', data: 'synthetic-state' }] };
    const sent: { payload: Record<string, unknown>; headers: Headers }[] = [];
    const lookups: string[] = [];
    const adapter: OpenRouterAdapter = new OpenRouterAdapter((): string => 'synthetic-key', async (url, init): Promise<Response> => {
      if (!init?.method) {
        lookups.push(String(url));
        return Response.json({ data: { endpoints: [{ provider_name: 'Synthetic upstream', tag: 'synthetic' }, { provider_name: 'Different provider', tag: 'other' }] } });
      }
      sent.push({ payload: JSON.parse(String(init.body)), headers: new Headers(init.headers) });
      return Response.json({ model: resolved, choices: [{ finish_reason: 'stop', message }], usage: { prompt_tokens: 14, completion_tokens: 3, prompt_tokens_details: { cached_tokens: 7, cache_write_tokens: 4 } }, openrouter_metadata: { endpoints: { available: [{ selected: true, provider: 'Synthetic upstream' }] } } });
    }, 'https://synthetic.example');
    const first: InferenceResult = await adapter.infer(input, target, AbortSignal.timeout(1000));
    expect(first.usage).toEqual({ input_tokens: 14, output_tokens: 3, cached_input_tokens: 7, cache_write_tokens: 4 });
    expect(first.continuation).toEqual({ connection_id: target.id, model: input.model, state: { format: 'openrouter_chat', message, upstream: 'synthetic', resolved_model: resolved } });
    const replay: InferenceRequest = { ...input, messages: [...input.messages, { role: 'assistant', content: first.content, continuation: first.continuation }] };
    const second: InferenceResult = await adapter.infer(replay, target, AbortSignal.timeout(1000));
    expect(second.continuation).toEqual(first.continuation);
    expect(sent[0]?.headers.get('x-openrouter-metadata')).toBe('enabled');
    expect(sent[1]?.payload).toMatchObject({
      model: resolved, messages: [...input.messages, message],
      provider: { require_parameters: true, only: ['synthetic'], allow_fallbacks: false },
      session_id: input.session_id, cache_control: { type: 'ephemeral' },
    });
    expect(lookups).toEqual(['https://synthetic.example/models/anthropic/resolved/endpoints']);
  });
  it('omits OpenRouter continuation when the selected upstream name matches several tags', async (): Promise<void> => {
    const target: Connection = connection('openrouter_http');
    const adapter: OpenRouterAdapter = new OpenRouterAdapter((): string => 'synthetic-key', async (_url, init): Promise<Response> => {
      if (init?.method === 'POST') return Response.json({ provider: 'Shared name', model: 'synthetic/model', choices: [{ finish_reason: 'stop', message: { content: 'Answer', reasoning: 'Synthetic reasoning' } }], usage: { prompt_tokens: 5, completion_tokens: 2 } });
      return Response.json({ data: { endpoints: [{ provider_name: 'Shared name', tag: 'one' }, { provider_name: 'Shared name', tag: 'two' }] } });
    }, 'https://synthetic.example');
    const result: InferenceResult = await adapter.infer(request(target), target, AbortSignal.timeout(1000));
    expect(result.content).toBe('Answer');
    expect(result).not.toHaveProperty('continuation');
    expect(result.usage).toEqual({ input_tokens: 5, output_tokens: 2 });
  });
  it('preserves OpenRouter tool metadata in continuation while returning normalized function calls', async (): Promise<void> => {
    const target: Connection = connection('openrouter_http');
    const call: ToolCall = { id: 'call-1', type: 'function', function: { name: 'lookup', arguments: '{}' } };
    const rawCall: ToolCall & { provider_metadata: { signature: string } } = { ...call, provider_metadata: { signature: 'synthetic' } };
    const adapter: OpenRouterAdapter = new OpenRouterAdapter((): string => 'synthetic-key', async (_url, init): Promise<Response> => {
      if (!init?.method) return Response.json({ data: { endpoints: [{ tag: 'synthetic' }] } });
      return Response.json({ provider: 'synthetic', model: 'synthetic/resolved', choices: [
        { finish_reason: 'tool_calls', message: { tool_calls: [rawCall], reasoning: 'Synthetic reasoning' } },
        { finish_reason: 'stop', message: { tool_calls: [null] } },
      ], usage: { prompt_tokens: 5, completion_tokens: 2 } });
    });
    const result: InferenceResult = await adapter.infer(request(target), target, AbortSignal.timeout(1000));
    expect(result.tool_calls).toEqual([call]);
    expect(result.continuation?.state).toMatchObject({ format: 'openrouter_chat', message: { tool_calls: [rawCall] } });
  });

  it.each([{ type: 'output_text' }, { type: 'output_text', text: 5 }])('rejects malformed Codex output_text after valid text (%j)', async (invalid): Promise<void> => {
    const target: Connection = connection('codex_http');
    const output: string = JSON.stringify([{ type: 'message', content: [{ type: 'output_text', text: 'Answer' }, invalid] }]);
    const adapter: CodexHttpAdapter = await syntheticCodex(async (): Promise<Response> => new Response(codexCompletion('{"input_tokens":5,"output_tokens":2}', output)));
    await expect(adapter.infer(request(target), target, AbortSignal.timeout(1000))).rejects.toMatchObject({ status: 502, code: 'PROVIDER_RESPONSE_INVALID' });
  });

  it('accepts empty Codex output text and ignores unrecognized message parts', async (): Promise<void> => {
    const target: Connection = connection('codex_http');
    const output: string = '[{"type":"message","content":[{"type":"output_text","text":""},{"type":"future_part","text":42},null]}]';
    const adapter: CodexHttpAdapter = await syntheticCodex(async (): Promise<Response> => new Response(codexCompletion('{"input_tokens":5,"output_tokens":2}', output)));
    await expect(adapter.infer(request(target), target, AbortSignal.timeout(1000))).resolves.toMatchObject({ content: '', continuation: { state: { items: JSON.parse(output) } } });
  });

  it('reports the first Codex output error even when later parts and usage are malformed', async (): Promise<void> => {
    const target: Connection = connection('codex_http');
    const output: string = '[{"type":"message","content":[{"type":"refusal","text":false},{"type":"output_text","text":9}]}]';
    const adapter: CodexHttpAdapter = await syntheticCodex(async (): Promise<Response> => new Response(codexCompletion('null', output)));
    await expect(adapter.infer(request(target), target, AbortSignal.timeout(1000))).rejects.toMatchObject({ status: 422, code: 'PROVIDER_REFUSAL' });
  });

  it('uses the fallback Codex output position when an event index is a float', async (): Promise<void> => {
    const target: Connection = connection('codex_http');
    const wire: string = `data:\n${[
      'data: {"type":"response.output_item.done","output_index":7.0,"item":{"type":"reasoning","encrypted_content":"synthetic"}}\n\n',
      codexCompletion('{"input_tokens":5,"output_tokens":2}', '[]'),
    ].join('')}`;
    const adapter: CodexHttpAdapter = await syntheticCodex(async (): Promise<Response> => new Response(wire));
    await expect(adapter.infer(request(target), target, AbortSignal.timeout(1000))).resolves.toMatchObject({ continuation: { state: { items: [{ type: 'reasoning', encrypted_content: 'synthetic' }] } } });
  });

  it('rejects malformed UTF-8 inside Codex event data', async (): Promise<void> => {
    const target: Connection = connection('codex_http');
    const wire: Buffer = Buffer.from(codexCompletion('{"input_tokens":5,"output_tokens":2}'));
    wire[wire.indexOf('Answer')] = 0xff;
    const adapter: CodexHttpAdapter = await syntheticCodex(async (): Promise<Response> => new Response(new Uint8Array(wire)));
    await expect(adapter.infer(request(target), target, AbortSignal.timeout(1000))).rejects.toMatchObject({ status: 502, code: 'PROVIDER_RESPONSE_INVALID' });
  });

  it('preserves separate Codex input and assistant text blocks, including empty blocks', async (): Promise<void> => {
    const target: Connection = connection('codex_http');
    const input: InferenceRequest = { ...request(target), messages: [
      { role: 'assistant', content: [{ type: 'text', text: 'Earlier' }, { type: 'text', text: '' }] },
      { role: 'user', content: [{ type: 'text', text: 'First' }, { type: 'text', text: 'Second' }] },
    ] };
    const sent: unknown[] = [];
    const adapter: CodexHttpAdapter = await syntheticCodex(async (_url, init): Promise<Response> => {
      sent.push(JSON.parse(String(init?.body)));
      return new Response(codexCompletion('{"input_tokens":5,"output_tokens":2}'));
    });
    await adapter.infer(input, target, AbortSignal.timeout(1000));
    expect(sent).toEqual([expect.objectContaining({ input: [
      { role: 'assistant', content: [{ type: 'output_text', text: 'Earlier' }, { type: 'output_text', text: '' }] },
      { role: 'user', content: [{ type: 'input_text', text: 'First' }, { type: 'input_text', text: 'Second' }] },
    ] })]);
  });

  it('uses native Codex Responses function items and requires an explicitly completed stream', async (): Promise<void> => {
    const target: Connection = connection('codex_http'); const input: InferenceRequest = { ...request(target), messages: [...request(target).messages, { role: 'assistant', content: '', tool_calls: [{ id: 'call1', type: 'function', function: { name: 'read_balance', arguments: '{"id":"a"}' } }] }, { role: 'tool', content: '{"balance":42}', tool_call_id: 'call1' }] };
    let sent: Record<string, unknown> = {};
    const home: string = await temporary(); const auth: CodexAuth = new CodexAuth(home, { load: async (): Promise<CodexCredentials> => credentials(Date.now() / 1000 + 3600), save: async (_expected, updated): Promise<CodexCredentials> => updated });
    const adapter: CodexHttpAdapter = new CodexHttpAdapter(async (_url, init): Promise<Response> => { sent = JSON.parse(String(init?.body)); return new Response(`event: response.completed\ndata: ${JSON.stringify({ type: 'response.completed', response: { status: 'completed', output: [{ type: 'message', content: [{ type: 'output_text', text: '{"balance":42}' }] }], usage: { input_tokens: 22, output_tokens: 9 } } })}\n\n`); }, 'https://synthetic.example', (): CodexAuth => auth);
    expect((await adapter.infer(input, target, AbortSignal.timeout(1000))).content).toBe('{"balance":42}');
    expect(sent).toMatchObject({ stream: true, store: false, input: expect.arrayContaining([{ type: 'function_call', call_id: 'call1', name: 'read_balance', arguments: '{"id":"a"}' }, { type: 'function_call_output', call_id: 'call1', output: '{"balance":42}' }]) });
    const incomplete: CodexHttpAdapter = new CodexHttpAdapter(async (): Promise<Response> => new Response('data: {"type":"response.incomplete"}\n\n'), 'https://synthetic.example', (): CodexAuth => auth);
    await expect(incomplete.infer(input, target, AbortSignal.timeout(1000))).rejects.toMatchObject({ code: 'INCOMPLETE_COMPLETION' });
  });
  it('decodes a completed subscription event across split UTF-8 and CRLF boundaries', async (): Promise<void> => {
    const target: Connection = connection('codex_http'); const home: string = await temporary();
    const auth: CodexAuth = new CodexAuth(home, { load: async (): Promise<CodexCredentials> => credentials(Date.now() / 1000 + 3600), save: async (_expected, updated): Promise<CodexCredentials> => updated });
    const wire: Buffer = Buffer.from(`data: ${JSON.stringify({ type: 'response.completed', response: { status: 'completed', output: [{ type: 'message', content: [{ type: 'output_text', text: '七' }] }], usage: { input_tokens: 5, output_tokens: 1 } } })}\r\n\r\n`);
    const adapter: CodexHttpAdapter = new CodexHttpAdapter(async (): Promise<Response> => new Response(new ReadableStream<Uint8Array>({ start(controller): void { for (const byte of wire) controller.enqueue(Uint8Array.of(byte)); controller.close(); } })), 'https://synthetic.example', (): CodexAuth => auth);
    expect((await adapter.infer(request(target), target, AbortSignal.timeout(1000))).content).toBe('七');
  });
  it('builds a CLI request isolated from user config, native tools and unrelated environment secrets', async (): Promise<void> => {
    if (process.platform === 'win32') return;
    const directory: string = await temporary(); const executable: string = path.join(directory, 'fake-codex'); const capture: string = path.join(directory, 'capture.json');
    // Model the stable CLI default: persistent goals and interactive input make a stateless invocation fail before inference.
    const source: string = `#!/usr/bin/env node\nconst fs=require('fs');const a=process.argv.slice(2);for(const feature of ['goals','default_mode_request_user_input'])if(!a.some((x,i)=>x==='--disable'&&a[i+1]===feature)){console.error('Interactive or persistent tools are unavailable in this ephemeral invocation.');process.exit(1);}fs.writeFileSync(${JSON.stringify(capture)},JSON.stringify({args:a,key:process.env.ROUTER_TEST_UNRELATED_SECRET,openai:process.env.OPENAI_API_KEY}));let s='';process.stdin.on('data',x=>s+=x);process.stdin.on('end',()=>{fs.writeFileSync(a[a.indexOf('-o')+1],JSON.stringify({decision:{kind:'tools',tool_calls:[{name:'read_balance',arguments:'{"id":"a"}'}]}}));console.log(JSON.stringify({type:'turn.completed',usage:{input_tokens:20,output_tokens:10}}));});`;
    await writeFile(executable, source, { mode: 0o700 });
    await writeFile(path.join(directory, 'models_cache.json'), JSON.stringify({ models: [{ slug: 'synthetic-model', context_window: 32000 }] }));
    const previous: string | undefined = process.env.ROUTER_TEST_UNRELATED_SECRET; process.env.ROUTER_TEST_UNRELATED_SECRET = 'must-not-inherit';
    try {
      const target: Connection = { ...connection('codex_http'), executable, codex_home: directory };
      const result: InferenceResult = await new CodexCliAdapter().infer(request(target), target, AbortSignal.timeout(3000));
      expect(result.tool_calls).toMatchObject([{ function: { name: 'read_balance' } }]);
      const captured: { args: string[]; key?: string; openai?: string } = JSON.parse(await readFile(capture, 'utf8'));
      expect(captured.key).toBeUndefined(); expect(captured.openai).toBeUndefined();
      for (const flag of ['--ephemeral', '--ignore-user-config', '--ignore-rules', '--sandbox', '--output-schema']) expect(captured.args).toContain(flag);
      for (const feature of ['shell_tool', 'multi_agent', 'apps', 'plugins', 'hooks', 'code_mode_host', 'computer_use', 'browser_use', 'view_image']) expect(captured.args).toContain(feature);
      const working: string = captured.args[captured.args.indexOf('-C') + 1]!; await expect(readFile(path.join(working, 'system.md'))).rejects.toThrow();
    } finally { if (previous === undefined) delete process.env.ROUTER_TEST_UNRELATED_SECRET; else process.env.ROUTER_TEST_UNRELATED_SECRET = previous; }
  });
});
it('omits GPT-Reserve from the Codex catalog while retaining ordinary models', (): void => {
  const models = codexCatalog({ models: [
    { slug: 'gpt-reserve', display_name: 'GPT-Reserve', context_window: 32000 },
    { slug: 'gpt-example', display_name: 'GPT Example', context_window: 64000 },
  ] });
  expect(models.map((model): string => model.id)).toEqual(['gpt-example']);
});
