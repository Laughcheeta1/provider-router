/** Provider wire contracts and OAuth rotation use deterministic synthetic endpoints. */
import { afterEach, describe, expect, it } from 'vitest';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { CodexAuth, ExistingCodexCredentials, codexAuthSettings, type CodexCredentials, type CredentialStore } from '../src/app/services/provider-service/providers/codex-auth.ts';
import { CodexHttpAdapter, OpenRouterAdapter } from '../src/app/services/provider-service/providers/http-providers.ts';
import { CodexCliAdapter } from '../src/app/services/provider-service/providers/codex-cli.ts';
import { uuid7 } from '../src/app/common/identity/identity.ts';
import type { Connection, InferenceRequest, InferenceResult } from '../src/app/common/schemas/contracts.ts';

const directories: string[] = [];
afterEach(async (): Promise<void> => { await Promise.all(directories.splice(0).map((directory: string): Promise<void> => rm(directory, { recursive: true, force: true }))); });
async function temporary(): Promise<string> { const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-provider-test-')); directories.push(directory); return directory; }
function connection(method: Connection['method']): Connection { return { id: uuid7(), name: 'Synthetic provider', method, exposed: true, models: [] }; }
function request(target: Connection): InferenceRequest { return { request_id: uuid7(), connection_id: target.id, model: 'synthetic-model', reasoning_effort: 'low', messages: [{ role: 'system', content: 'Synthetic standing instruction.' }, { role: 'user', content: 'Synthetic question.' }], tools: [] }; }
function credentials(expiration: number): CodexCredentials { return { auth_mode: 'chatgpt', last_refresh: new Date().toISOString(), tokens: { access_token: `header.${Buffer.from(JSON.stringify({ exp: expiration })).toString('base64url')}.signature`, refresh_token: 'synthetic-refresh', id_token: 'synthetic-identity', account_id: 'synthetic-account', login_owner_id: 'synthetic-login' } }; }

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
    const source: string = `#!/usr/bin/env node\nconst fs=require('fs');const a=process.argv.slice(2);fs.writeFileSync(${JSON.stringify(capture)},JSON.stringify({args:a,key:process.env.ROUTER_TEST_UNRELATED_SECRET,openai:process.env.OPENAI_API_KEY}));let s='';process.stdin.on('data',x=>s+=x);process.stdin.on('end',()=>{fs.writeFileSync(a[a.indexOf('-o')+1],JSON.stringify({decision:{kind:'tools',tool_calls:[{name:'read_balance',arguments:'{"id":"a"}'}]}}));console.log(JSON.stringify({type:'turn.completed',usage:{input_tokens:20,output_tokens:10}}));});`;
    await writeFile(executable, source, { mode: 0o700 });
    await writeFile(path.join(directory, 'models_cache.json'), JSON.stringify({ models: [{ slug: 'synthetic-model', context_window: 32000 }] }));
    const previous: string | undefined = process.env.ROUTER_TEST_UNRELATED_SECRET; process.env.ROUTER_TEST_UNRELATED_SECRET = 'must-not-inherit';
    try {
      const target: Connection = { ...connection('codex_cli'), executable, codex_home: directory };
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
