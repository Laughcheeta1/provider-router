/** Exercise the actual authenticated HTTP boundary and cancellation ownership. */
import { afterEach, describe, expect, it } from 'vitest';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { RouterStore, type SecretCipher } from '../src/app/products/router/repository/settings/file.ts';
import { startRouter, validateNetwork, type ListeningRouter } from '../src/app/products/router/router.ts';
import { uuid7 } from '../src/app/common/identity/identity.ts';
import type { Connection, InferenceRequest, InferenceResult, Model, ProviderAdapter } from '../src/app/common/schemas/contracts.ts';

const directories: string[] = [];
const listeners: ListeningRouter[] = [];
const model: Model = { id: 'test-model', name: 'Synthetic model', reasoning_efforts: ['low', 'high'], default_reasoning_effort: 'low', context_window: 32000, max_output_tokens: 4000, tools: true, structured_output: true, input_modalities: ['text'] };
const cipher: SecretCipher = { encrypt: (value: string): Buffer => Buffer.from(`encrypted:${value}`).reverse(), decrypt: (value: Buffer): string => Buffer.from(value).reverse().toString().slice(10) };

afterEach(async (): Promise<void> => { await Promise.all(listeners.splice(0).map((item: ListeningRouter): Promise<void> => item.close())); await Promise.all(directories.splice(0).map((directory: string): Promise<void> => rm(directory, { recursive: true, force: true }))); });

async function fixture(infer?: ProviderAdapter['infer']): Promise<{ store: RouterStore; listener: ListeningRouter; token: string; client_id: string; connection: Connection; directory: string }> {
  const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-test-')); directories.push(directory);
  const store: RouterStore = await RouterStore.open(directory, cipher);
  const connection: Connection = { id: uuid7(), name: 'Personal subscription', method: 'codex_cli', exposed: true, models: [model] };
  await store.saveConnection(connection, 'upstream-secret');
  const client: { id: string; token: string } = await store.createClient('Example application');
  const adapter: ProviderAdapter = { models: async (): Promise<Model[]> => [model], infer: infer || (async (request: InferenceRequest): Promise<InferenceResult> => ({ request_id: request.request_id, content: '{"answer":42}', tool_calls: [], usage: { input_tokens: 20, output_tokens: 8 }, provider: 'codex', method: 'codex_cli', model: request.model, reasoning_effort: request.reasoning_effort })) };
  const listener: ListeningRouter = await startRouter(store, { codex_cli: adapter, codex_http: adapter, openrouter_http: adapter }, { host: '127.0.0.1', port: 0, remote_enabled: false, trusted_proxy: false }, '0.1.0'); listeners.push(listener);
  return { store, listener, token: client.token, client_id: client.id, connection, directory };
}

function request(connection: Connection): InferenceRequest { return { request_id: uuid7(), connection_id: connection.id, model: model.id, reasoning_effort: 'low', messages: [{ role: 'user', content: 'Synthetic question' }], tools: [] }; }
async function post(endpoint: string, token: string, payload: unknown): Promise<Response> { return fetch(`${endpoint}/v1/inference`, { method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' }, body: JSON.stringify(payload) }); }

describe('Provider Router API', (): void => {
  it('discovers identity publicly and requires a client key before exposing connections', async (): Promise<void> => {
    const { listener, token, connection, store, directory } = await fixture();
    expect(await (await fetch(`${listener.endpoint}/health`)).json()).toMatchObject({ name: 'provider-router', protocol_version: 1, instance_id: store.publicState().instance_id });
    expect((await fetch(`${listener.endpoint}/v1/providers`)).status).toBe(401);
    const visible: string = await (await fetch(`${listener.endpoint}/v1/providers`, { headers: { Authorization: `Bearer ${token}` } })).text();
    expect(JSON.parse(visible)).toMatchObject({ providers: [{ id: connection.id, models: [{ reasoning_efforts: ['low', 'high'] }] }] });
    expect(visible).not.toContain('upstream-secret'); expect(visible).not.toContain('token_hash');
    const persisted: string = await readFile(path.join(directory, 'settings.json'), 'utf8'); expect(persisted).not.toContain('upstream-secret'); expect(persisted).not.toContain(token);
    const restored: RouterStore = await RouterStore.open(directory, cipher); expect(restored.secret(connection.id)).toBe('upstream-secret'); expect(restored.authenticate(token)?.name).toBe('Example application');
  });

  it('returns a valid completion, rejects replay, invalid choices and unrepresented media', async (): Promise<void> => {
    const { listener, token, connection } = await fixture(); const payload: InferenceRequest = request(connection);
    expect(await (await post(listener.endpoint, token, payload)).json()).toMatchObject({ request_id: payload.request_id, content: '{"answer":42}' });
    expect((await post(listener.endpoint, token, payload)).status).toBe(409);
    for (const invalid of [{ model: 'unknown' }, { reasoning_effort: 'ultra' }, { messages: [{ role: 'user', content: [{ type: 'image_url', image_url: { url: 'https://example.com/image' } }] }] }]) expect((await post(listener.endpoint, token, { ...request(connection), ...invalid })).status).toBe(400);
  });

  it('validates returned tool arguments and answer schema before publishing', async (): Promise<void> => {
    const { listener, token, connection } = await fixture();
    const payload: InferenceRequest = { ...request(connection), response_format: { type: 'json_schema', json_schema: { name: 'answer', schema: { type: 'object', properties: { answer: { type: 'string' } }, required: ['answer'] } } } };
    expect((await post(listener.endpoint, token, payload)).status).toBe(502);
    const toolsFixture = await fixture(async (input: InferenceRequest): Promise<InferenceResult> => ({ request_id: input.request_id, model: input.model, content: '', tool_calls: [{ id: 'call-1', type: 'function', function: { name: 'read_balance', arguments: '{"id":12}' } }], usage: { input_tokens: 1, output_tokens: 1 }, provider: 'codex', method: 'codex_cli' }));
    const toolRequest: InferenceRequest = { ...request(toolsFixture.connection), tools: [{ type: 'function', function: { name: 'read_balance', parameters: { type: 'object', properties: { id: { type: 'string' } }, required: ['id'] } } }] };
    expect((await post(toolsFixture.listener.endpoint, toolsFixture.token, toolRequest)).status).toBe(502);
  });

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
  });

  it('stops provider execution when its HTTP caller disconnects', async (): Promise<void> => {
    let started: () => void = (): void => {}; const ready: Promise<void> = new Promise((resolve): void => { started = resolve; });
    let finish: () => void = (): void => {}; const cancelled: Promise<void> = new Promise((resolve): void => { finish = resolve; });
    const { listener, token, connection } = await fixture(async (_input, _connection, signal): Promise<InferenceResult> => new Promise((_resolve, reject): void => { started(); signal.addEventListener('abort', (): void => { finish(); reject(new Error('cancelled')); }, { once: true }); }));
    const controller: AbortController = new AbortController();
    const pending: Promise<Response> = fetch(`${listener.endpoint}/v1/inference`, { method: 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' }, body: JSON.stringify(request(connection)), signal: controller.signal });
    await ready; controller.abort(); await expect(pending).rejects.toThrow(); await cancelled;
  });

  it('rejects browser-origin requests and hides unexposed connections', async (): Promise<void> => {
    const { listener, token, connection, store } = await fixture(); await store.saveConnection({ ...connection, exposed: false });
    expect((await post(listener.endpoint, token, request(connection))).status).toBe(404);
    expect((await fetch(`${listener.endpoint}/v1/providers`, { headers: { Origin: 'https://untrusted.example', Authorization: `Bearer ${token}` } })).status).toBe(403);
  });

  it('rejects malformed text blocks and unmatched tool results before calling a provider', async (): Promise<void> => {
    let calls: number = 0;
    const { listener, token, connection } = await fixture(async (): Promise<InferenceResult> => { calls++; throw new Error('Invalid input must not reach the provider.'); });
    for (const messages of [[{ role: 'user', content: [{ type: 'text', text: 123 }] }], [{ role: 'tool', tool_call_id: 'unknown', content: 'Synthetic result' }]]) expect((await post(listener.endpoint, token, { ...request(connection), messages })).status).toBe(400);
    expect(calls).toBe(0);
  });
});

describe('local configuration writes', (): void => {
  it('leaves no partial connection when the credential vault rejects a key', async (): Promise<void> => {
    const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-vault-test-')); directories.push(directory);
    const store: RouterStore = await RouterStore.open(directory, { encrypt: (): never => { throw new Error('Vault unavailable'); }, decrypt: (): never => { throw new Error('Vault unavailable'); } });
    await expect(store.saveConnection({ id: uuid7(), name: 'OpenRouter', method: 'openrouter_http', exposed: false, models: [] }, 'synthetic-secret')).rejects.toThrow('Vault unavailable');
    expect(store.connections()).toEqual([]);
  });

  it('persists concurrent independent connection changes without losing either', async (): Promise<void> => {
    const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-write-test-')); directories.push(directory);
    const store: RouterStore = await RouterStore.open(directory, cipher);
    const one: Connection = { id: uuid7(), name: 'One', method: 'codex_cli', exposed: false, models: [model] };
    const two: Connection = { id: uuid7(), name: 'Two', method: 'codex_http', exposed: false, models: [model] };
    await Promise.all([store.saveConnection(one), store.saveConnection(two)]);
    expect((await RouterStore.open(directory, cipher)).connections().map((item: Connection): string => item.name)).toEqual(['One', 'Two']);
  });
});

describe('remote configuration', (): void => {
  it('requires explicit remote mode and TLS or a loopback trusted HTTPS proxy', (): void => {
    expect((): void => validateNetwork({ host: '0.0.0.0', port: 47831, remote_enabled: false, trusted_proxy: false })).toThrow();
    expect((): void => validateNetwork({ host: '0.0.0.0', port: 47831, remote_enabled: true, trusted_proxy: false })).toThrow();
    expect((): void => validateNetwork({ host: '0.0.0.0', port: 47831, remote_enabled: true, trusted_proxy: true, public_endpoint: 'https://providers.example' })).toThrow();
    expect((): void => validateNetwork({ host: '127.0.0.1', port: 47831, remote_enabled: true, trusted_proxy: true, public_endpoint: 'https://providers.example' })).not.toThrow();
  });
});
