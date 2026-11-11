/** Exercise the actual authenticated HTTP boundary and cancellation ownership. */
import { afterEach, describe, expect, it } from 'vitest';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import { request as httpRequest, type IncomingMessage } from 'node:http';
import path from 'node:path';
import { RouterStore, type SecretCipher } from '../src/app/products/router/repository/settings/file.ts';
import { startRouter, validateNetwork, type ListeningRouter } from '../src/app/products/router/router.ts';
import { uuid7 } from '../src/app/common/identity/identity.ts';
import type { Connection, InferenceRequest, InferenceResult, Model, ProviderAdapter } from '../src/app/common/schemas/contracts.ts';
// RevExt: 1
const directories: string[] = [];
const listeners: ListeningRouter[] = [];
const model: Model = { id: 'test-model', name: 'Synthetic model', reasoning_efforts: ['low', 'high'], default_reasoning_effort: 'low', context_window: 32000, max_output_tokens: 4000, tools: true, structured_output: true, input_modalities: ['text'] };
const cipher: SecretCipher = { encrypt: (value: string): Buffer => Buffer.from(`encrypted:${value}`).reverse(), decrypt: (value: Buffer): string => Buffer.from(value).reverse().toString().slice(10) };
// RevExt: 2
afterEach(async (): Promise<void> => { await Promise.all(listeners.splice(0).map((item: ListeningRouter): Promise<void> => item.close())); await Promise.all(directories.splice(0).map((directory: string): Promise<void> => rm(directory, { recursive: true, force: true }))); });
// RevExt: 3
async function fixture(infer?: ProviderAdapter['infer'], requireApiKey: boolean = true, browserOrigins: string[] = []): Promise<{ store: RouterStore; listener: ListeningRouter; token: string; client_id: string; connection: Connection; directory: string }> {
  const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-test-')); directories.push(directory);
  const store: RouterStore = await RouterStore.open(directory, cipher);  // RevExt: 59
  const connection: Connection = { id: uuid7(), name: 'Personal subscription', method: 'codex_http', exposed: true, models: [model] };
  await store.saveConnection(connection, 'upstream-secret');
  const client: { id: string; token: string } = await store.createClient('Example application');
  const adapter: ProviderAdapter = { models: async (): Promise<Model[]> => [model], infer: infer || (async (request: InferenceRequest): Promise<InferenceResult> => ({ request_id: request.request_id, content: '{"answer":42}', tool_calls: [], usage: { input_tokens: 20, output_tokens: 8 }, provider: 'codex', method: 'codex_http', model: request.model, reasoning_effort: request.reasoning_effort })) };
  const listener: ListeningRouter = await startRouter(store, { codex_cli: adapter, codex_http: adapter, openrouter_http: adapter }, { host: '127.0.0.1', port: 0, require_api_key: requireApiKey, browser_origins: browserOrigins }, '0.1.0'); listeners.push(listener);
  return { store, listener, token: client.token, client_id: client.id, connection, directory };
}
// RevExt: 4
function request(connection: Connection): InferenceRequest { return { request_id: uuid7(), connection_id: connection.id, model: model.id, reasoning_effort: 'low', messages: [{ role: 'user', content: 'Synthetic question' }], tools: [] }; }
async function post(endpoint: string, token: string, payload: unknown): Promise<Response> { return fetch(`${endpoint}/v1/inference`, { method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' }, body: JSON.stringify(payload) }); }
// RevExt: 5
describe('Provider Router API', (): void => {
  it('allows approved browser origins through preflight and inference without a key, and keeps errors readable', async (): Promise<void> => {
    const origin: string = 'https://attalid.example';  // RevExt: 37
    const { listener, connection } = await fixture(undefined, false, [origin]);
    const preflight: Response = await fetch(`${listener.endpoint}/v1/inference`, { method: 'OPTIONS', headers: { Origin: origin, 'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'content-type, authorization, x-request-id' } });
    expect(preflight.status).toBe(204);
    expect(preflight.headers.get('access-control-allow-origin')).toBe(origin);
    expect(preflight.headers.get('access-control-allow-headers')).toContain('authorization');
    const connectivity: Response = await fetch(`${listener.endpoint}/v1/connectivity`, { headers: { Origin: origin } });
    expect(await connectivity.json()).toMatchObject({ name: 'provider-router', authentication_required: false, ready: true, providers: 1, models: 1 });
    expect(connectivity.headers.get('access-control-allow-origin')).toBe(origin);
    const response: Response = await fetch(`${listener.endpoint}/v1/inference`, { method: 'POST', headers: { Origin: origin, 'Content-Type': 'application/json' }, body: JSON.stringify(request(connection)) });
    expect(response.status).toBe(200);  // RevExt: 40
    expect(response.headers.get('access-control-allow-origin')).toBe(origin);
    const missing: Response = await fetch(`${listener.endpoint}/unknown`, { headers: { Origin: origin } });
    expect(missing.status).toBe(404);
    expect(missing.headers.get('access-control-allow-origin')).toBe(origin);
  });  // RevExt: 42
// RevExt: 34
  it('enforces browser approval before every endpoint and restricts preflight methods and headers', async (): Promise<void> => {
    const origin: string = 'https://attalid.example';  // RevExt: 38
    const { listener } = await fixture(undefined, false, [origin]);
    for (const rejectedOrigin of ['null', 'https://untrusted.example', `${origin}.evil.example`, `${origin}/path`]) {
      for (const endpoint of ['/health', '/v1/connectivity', '/v1/providers', '/v1/models', '/v1/inference']) {
        const response: Response = await fetch(`${listener.endpoint}${endpoint}`, { headers: { Origin: rejectedOrigin } });
        expect(response.status).toBe(403);
        expect(response.headers.get('access-control-allow-origin')).toBeNull();
      }
    }  // RevExt: 45
    const rejectedPreflights: Record<string, string>[] = [{ 'Access-Control-Request-Method': 'DELETE' }, { 'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'x-custom-header' }];
    for (const headers of rejectedPreflights) {
      expect((await fetch(`${listener.endpoint}/v1/inference`, { method: 'OPTIONS', headers: { Origin: origin, ...headers } })).status).toBe(403);
    }  // RevExt: 46
  });  // RevExt: 43
// RevExt: 35
  it('tests the client key and exposed model readiness without calling a provider', async (): Promise<void> => {
    let calls: number = 0;  // RevExt: 48
    const origin: string = 'https://attalid.example';  // RevExt: 39
    const { listener, connection, store, token } = await fixture(async (): Promise<never> => { calls++; throw new Error('The connectivity check must not run inference.'); }, true, [origin]);
    const unauthorized: Response = await fetch(`${listener.endpoint}/v1/connectivity`, { headers: { Origin: origin } });
    expect(unauthorized.status).toBe(401);
    expect(unauthorized.headers.get('access-control-allow-origin')).toBe(origin);
    expect(await unauthorized.json()).toMatchObject({ error: { code: 'CLIENT_UNAUTHORIZED' } });
    expect((await fetch(`${listener.endpoint}/v1/connectivity`, { headers: { Authorization: `Bearer ${token}`, Origin: origin } })).status).toBe(200);
    await store.saveConnection({ ...connection, exposed: false });
    expect(await (await fetch(`${listener.endpoint}/v1/connectivity`, { headers: { Authorization: `Bearer ${token}`, Origin: origin } })).json()).toMatchObject({ ready: false, providers: 0, models: 0 });
    expect(calls).toBe(0);  // RevExt: 50
  });  // RevExt: 44
// RevExt: 36
  it('serves discovery and inference without a key when authentication is disabled, while rejecting browser origins and rebinding hosts', async (): Promise<void> => {
    const { listener, connection } = await fixture(undefined, false);
    expect(await (await fetch(`${listener.endpoint}/health`)).json()).toMatchObject({ authentication_required: false });
    expect((await fetch(`${listener.endpoint}/v1/providers`)).status).toBe(200);
    const catalogue = await (await fetch(`${listener.endpoint}/v1/models`)).json() as { data: { provider_router: Record<string, unknown> }[] };
    expect(catalogue.data[0]?.provider_router).toMatchObject({ connection_id: connection.id, connection_name: connection.name, reasoning_efforts: ['low', 'high'], tools: true, structured_output: true });
    const response: Response = await fetch(`${listener.endpoint}/v1/inference`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(request(connection)) });
    expect(response.status).toBe(200);  // RevExt: 41
    expect((await fetch(`${listener.endpoint}/v1/models`, { headers: { Origin: 'https://unrelated.example' } })).status).toBe(403);
    const rebound: number | undefined = await new Promise<number | undefined>((resolve, reject): void => {
      const call = httpRequest(`${listener.endpoint}/v1/models`, { headers: { Host: 'unrelated.example' } }, (response: IncomingMessage): void => { response.resume(); resolve(response.statusCode); });
      call.on('error', reject); call.end();
    });
    expect(rebound).toBe(403);
    expect((await fetch(`${listener.endpoint}/v1/models`, { headers: { Authorization: 'Bearer unused-sdk-placeholder' } })).status).toBe(200);
  });  // RevExt: 18
// RevExt: 6
  it('uses unauthenticated loopback defaults for new profiles and preserves authentication for old profiles', async (): Promise<void> => {
    const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-migration-')); directories.push(directory);
    const fresh: RouterStore = await RouterStore.open(directory, cipher);
    expect(fresh.publicState().network).toMatchObject({ host: '127.0.0.1', port: 47831, require_api_key: false });
    await fresh.saveNetwork(fresh.publicState().network);
    const file: string = path.join(directory, 'settings.json');  // RevExt: 56
    const stored = JSON.parse(await readFile(file, 'utf8')) as { network: Record<string, unknown> };
    delete stored.network.require_api_key;
    await writeFile(file, JSON.stringify(stored));
    expect((await RouterStore.open(directory, cipher)).publicState().network.require_api_key).toBe(true);
  });  // RevExt: 19
// RevExt: 7
  it('applies the configured allowed default effort and rejects a disabled effort before provider execution', async (): Promise<void> => {
    const observed: (string | undefined)[] = [];
    const { listener, connection, store } = await fixture(async (input: InferenceRequest): Promise<InferenceResult> => {
      observed.push(input.reasoning_effort);
      return { request_id: input.request_id, content: 'Hello', tool_calls: [], usage: { input_tokens: 1, output_tokens: 1 }, provider: 'codex', method: 'codex_http', model: input.model, reasoning_effort: input.reasoning_effort };
    }, false);
    await store.saveConnection({ ...connection, models: [{ ...model, reasoning_efforts: ['high'], default_reasoning_effort: 'high' }] });
    const input: InferenceRequest = request(connection); delete input.reasoning_effort;
    const send = (value: InferenceRequest): Promise<Response> => fetch(`${listener.endpoint}/v1/inference`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(value) });
    expect((await send(input)).status).toBe(200);
    expect(observed).toEqual(['high']);  // RevExt: 31
    expect((await send(request(connection))).status).toBe(400);
    expect(observed).toEqual(['high']);  // RevExt: 32
  });  // RevExt: 20
// RevExt: 8
  it('discovers identity publicly and requires a client key before exposing connections', async (): Promise<void> => {
    const { listener, token, connection, store, directory } = await fixture();
    expect(await (await fetch(`${listener.endpoint}/health`)).json()).toMatchObject({ name: 'provider-router', protocol_version: 1, instance_id: store.publicState().instance_id });
    expect((await fetch(`${listener.endpoint}/v1/providers`)).status).toBe(401);
    for (const invalid of ['local', 'invalid', 'x'.repeat(43)]) {
      const rejected: Response = await fetch(`${listener.endpoint}/v1/providers`, { headers: { Authorization: `Bearer ${invalid}` } });
      expect(rejected.status).toBe(401);
      expect(await rejected.json()).toMatchObject({ error: { code: 'CLIENT_UNAUTHORIZED' } });
      expect((await post(listener.endpoint, invalid, request(connection))).status).toBe(401);
    }  // RevExt: 47
// RevExt: 33
    const visible: string = await (await fetch(`${listener.endpoint}/v1/providers`, { headers: { Authorization: `Bearer ${token}` } })).text();
    expect(JSON.parse(visible)).toMatchObject({ providers: [{ id: connection.id, models: [{ reasoning_efforts: ['low', 'high'] }] }] });
    expect(visible).not.toContain('upstream-secret'); expect(visible).not.toContain('token_hash');
    const persisted: string = await readFile(path.join(directory, 'settings.json'), 'utf8'); expect(persisted).not.toContain('upstream-secret'); expect(persisted).not.toContain(token);
    const restored: RouterStore = await RouterStore.open(directory, cipher); expect(restored.secret(connection.id)).toBe('upstream-secret'); expect(restored.authenticate(token)?.name).toBe('Example application');
  });  // RevExt: 21
// RevExt: 9
  it('returns a valid completion, rejects replay, invalid choices and unrepresented media', async (): Promise<void> => {
    const { listener, token, connection } = await fixture(); const payload: InferenceRequest = request(connection);
    expect(await (await post(listener.endpoint, token, payload)).json()).toMatchObject({ request_id: payload.request_id, content: '{"answer":42}' });
    expect((await post(listener.endpoint, token, payload)).status).toBe(409);
    for (const invalid of [{ model: 'unknown' }, { reasoning_effort: 'ultra' }, { messages: [{ role: 'user', content: [{ type: 'image_url', image_url: { url: 'https://example.com/image' } }] }] }]) expect((await post(listener.endpoint, token, { ...request(connection), ...invalid })).status).toBe(400);
  });  // RevExt: 22
// RevExt: 10
  it('validates returned tool arguments and answer schema before publishing', async (): Promise<void> => {
    const { listener, token, connection } = await fixture();
    const payload: InferenceRequest = { ...request(connection), response_format: { type: 'json_schema', json_schema: { name: 'answer', schema: { type: 'object', properties: { answer: { type: 'string' } }, required: ['answer'] } } } };
    expect((await post(listener.endpoint, token, payload)).status).toBe(502);
    const toolsFixture = await fixture(async (input: InferenceRequest): Promise<InferenceResult> => ({ request_id: input.request_id, model: input.model, content: '', tool_calls: [{ id: 'call-1', type: 'function', function: { name: 'read_balance', arguments: '{"id":12}' } }], usage: { input_tokens: 1, output_tokens: 1 }, provider: 'codex', method: 'codex_http' }));
    const toolRequest: InferenceRequest = { ...request(toolsFixture.connection), tools: [{ type: 'function', function: { name: 'read_balance', parameters: { type: 'object', properties: { id: { type: 'string' } }, required: ['id'] } } }] };
    expect((await post(toolsFixture.listener.endpoint, toolsFixture.token, toolRequest)).status).toBe(502);
  });  // RevExt: 23
// RevExt: 11
  it('cancels only the originating client request and revokes active work', async (): Promise<void> => {
    let started: () => void = (): void => {}; const ready: Promise<void> = new Promise((resolve): void => { started = resolve; });
    let cancelled: boolean = false;
    const { listener, token, connection, store, client_id } = await fixture(async (_input, _connection, signal): Promise<InferenceResult> => { started(); return new Promise((_resolve, reject): void => { signal.addEventListener('abort', (): void => { cancelled = true; reject(new Error('cancelled')); }, { once: true }); }); });
    const payload: InferenceRequest = request(connection); const pending: Promise<Response> = post(listener.endpoint, token, payload); await ready;
    const other: { id: string; token: string } = await store.createClient('Other application');
    expect((await fetch(`${listener.endpoint}/v1/inference/${payload.request_id}/cancel`, { method: 'POST', headers: { Authorization: `Bearer ${other.token}` } })).status).toBe(404);
    expect(cancelled).toBe(false);
    await listener.revokeClient(client_id); expect((await pending).status).toBe(499); expect(cancelled).toBe(true);
    expect((await post(listener.endpoint, token, request(connection))).status).toBe(401);
  });  // RevExt: 24
// RevExt: 12
  it('stops provider execution when its HTTP caller disconnects', async (): Promise<void> => {
    let started: () => void = (): void => {}; const ready: Promise<void> = new Promise((resolve): void => { started = resolve; });
    let finish: () => void = (): void => {}; const cancelled: Promise<void> = new Promise((resolve): void => { finish = resolve; });
    const { listener, token, connection } = await fixture(async (_input, _connection, signal): Promise<InferenceResult> => new Promise((_resolve, reject): void => { started(); signal.addEventListener('abort', (): void => { finish(); reject(new Error('cancelled')); }, { once: true }); }));
    const controller: AbortController = new AbortController();
    const pending: Promise<Response> = fetch(`${listener.endpoint}/v1/inference`, { method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' }, body: JSON.stringify(request(connection)), signal: controller.signal });
    await ready; controller.abort(); await expect(pending).rejects.toThrow(); await cancelled;
  });  // RevExt: 25
// RevExt: 13
  it('rejects browser-origin requests and hides unexposed connections', async (): Promise<void> => {
    const { listener, token, connection, store } = await fixture(); await store.saveConnection({ ...connection, exposed: false });
    expect((await post(listener.endpoint, token, request(connection))).status).toBe(404);
    expect((await fetch(`${listener.endpoint}/v1/providers`, { headers: { Origin: 'https://untrusted.example', Authorization: `Bearer ${token}` } })).status).toBe(403);
  });  // RevExt: 26
// RevExt: 14
  it('rejects malformed text blocks and unmatched tool results before calling a provider', async (): Promise<void> => {
    let calls: number = 0;  // RevExt: 49
    const { listener, token, connection } = await fixture(async (): Promise<InferenceResult> => { calls++; throw new Error('Invalid input must not reach the provider.'); });
    for (const messages of [[{ role: 'user', content: [{ type: 'text', text: 123 }] }], [{ role: 'tool', tool_call_id: 'unknown', content: 'Synthetic result' }]]) expect((await post(listener.endpoint, token, { ...request(connection), messages })).status).toBe(400);
    expect(calls).toBe(0);  // RevExt: 51
  });  // RevExt: 27
});  // RevExt: 61
// RevExt: 15
describe('local configuration writes', (): void => {
  it('leaves no partial connection when the credential vault rejects a key', async (): Promise<void> => {
    const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-vault-test-')); directories.push(directory);
    const store: RouterStore = await RouterStore.open(directory, { encrypt: (): never => { throw new Error('Vault unavailable'); }, decrypt: (): never => { throw new Error('Vault unavailable'); } });
    await expect(store.saveConnection({ id: uuid7(), name: 'OpenRouter', method: 'openrouter_http', exposed: false, models: [] }, 'synthetic-secret')).rejects.toThrow('Vault unavailable');
    expect(store.connections()).toEqual([]);
  });  // RevExt: 28
// RevExt: 16
  it('persists concurrent independent connection changes without losing either', async (): Promise<void> => {
    const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-write-test-')); directories.push(directory);
    const store: RouterStore = await RouterStore.open(directory, cipher);
    const one: Connection = { id: uuid7(), name: 'One', method: 'codex_http', exposed: false, models: [model] };
    const two: Connection = { id: uuid7(), name: 'Two', method: 'codex_http', exposed: false, models: [model] };
    await Promise.all([store.saveConnection(one), store.saveConnection(two)]);
    expect((await RouterStore.open(directory, cipher)).connections().map((item: Connection): string => item.name)).toEqual(['One', 'Two']);
  });  // RevExt: 29
});  // RevExt: 62
// RevExt: 17
describe('local-only configuration', (): void => {
  it('rejects network listeners and accepts loopback listeners', (): void => {
    for (const host of ['0.0.0.0', '::', '192.168.1.10', 'router.example']) {
      expect((): void => validateNetwork({ host, port: 47831, require_api_key: true })).toThrow('only supports loopback');
    }  // RevExt: 54
    for (const host of ['127.0.0.1', '::1', 'localhost']) {
      expect((): void => validateNetwork({ host, port: 47831, require_api_key: false })).not.toThrow();
    }  // RevExt: 55
  });  // RevExt: 30
});  // RevExt: 63
// RevExt: 52
  it('retires saved remote settings without losing local configuration or client keys', async (): Promise<void> => {
    const { store, directory, token } = await fixture();
    const file: string = path.join(directory, 'settings.json');  // RevExt: 57
    const saved: Record<string, unknown> = JSON.parse(await readFile(file, 'utf8')) as Record<string, unknown>;
    saved.network = { host: '0.0.0.0', port: 48901, require_api_key: true, browser_origins: ['https://app.example'], remote_enabled: true, trusted_proxy: true, public_endpoint: 'https://router.example', tls_certificate: '/missing/cert', tls_key: '/missing/key' };
    await writeFile(file, JSON.stringify(saved));
    const reopened: RouterStore = await RouterStore.open(directory, cipher);
    expect(reopened.publicState().network).toEqual({ host: '127.0.0.1', port: 48901, require_api_key: true, browser_origins: ['https://app.example'] });
    expect(reopened.authenticate(token)?.id).toBe(store.authenticate(token)?.id);
    await expect(reopened.saveNetwork({ host: '0.0.0.0', port: 48901, require_api_key: true })).rejects.toThrow();
  });  // RevExt: 53
// RevExt: 58
it('removes GPT-Reserve from saved Codex selections while preserving OpenRouter selections', async (): Promise<void> => {
  const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-model-selection-')); directories.push(directory);
  const store: RouterStore = await RouterStore.open(directory, cipher);  // RevExt: 60
  const reserve: Model = { ...model, id: 'gpt-reserve', name: 'GPT-Reserve' };
  for (const method of ['codex_cli', 'codex_http', 'openrouter_http'] as const) {
    await store.saveConnection({ id: uuid7(), name: method, method, exposed: true, models: [reserve, model] });
  }
  const saved: Connection[] = (await RouterStore.open(directory, cipher)).connections();
  expect(saved.map((connection: Connection): string[] => connection.models.map((entry: Model): string => entry.id))).toEqual([[model.id], [model.id], ['gpt-reserve', model.id]]);
  const file: string = path.join(directory, 'settings.json');
  const legacy: Record<string, unknown> = JSON.parse(await readFile(file, 'utf8')) as Record<string, unknown>;
  legacy.connections = saved.map((connection: Connection): Connection => ({ ...connection, models: [reserve, model] }));
  await writeFile(file, JSON.stringify(legacy));
  expect((await RouterStore.open(directory, cipher)).connections().map((connection: Connection): number => connection.models.length)).toEqual([1, 1, 2]);
});  // RevExt: 64
