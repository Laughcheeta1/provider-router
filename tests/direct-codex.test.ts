/** Upgrade a saved connection and exercise the browser-to-subscription HTTP contract. */
import { expect, it } from 'vitest';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { uuid7 } from '../src/app/common/identity/identity.ts';
import type { Model } from '../src/app/common/schemas/contracts.ts';
import { RouterStore } from '../src/app/products/router/repository/settings/file.ts';
import { startRouter, type ListeningRouter } from '../src/app/products/router/router.ts';
import { CodexHttpAdapter } from '../src/app/services/provider-service/providers/http-providers.ts';

it('migrates CLI identities and sends only caller input directly to the subscription API', async (): Promise<void> => {
  const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-direct-'));
  let listener: ListeningRouter | undefined;
  try {
    const id: string = uuid7();
    const model: Model = { id: 'gpt-6.1-sol', name: 'Sol', reasoning_efforts: ['low'], default_reasoning_effort: 'low', context_window: 500000, max_output_tokens: 100000, tools: true, structured_output: true, input_modalities: ['text'] };
    const original: Record<string, unknown> = { id, name: 'Existing subscription', method: 'codex_cli', exposed: true, models: [model], codex_home: directory, environment: 'native' };
    await writeFile(path.join(directory, 'settings.json'), JSON.stringify({ version: 1, instance_id: uuid7(), connections: [original], clients: [], secrets: {}, network: { require_api_key: false, browser_origins: ['https://antikythera.example'] } }));
    const access: string = `header.${Buffer.from(JSON.stringify({ exp: Date.now() / 1000 + 3600 })).toString('base64url')}.signature`;
    await writeFile(path.join(directory, 'auth.json'), JSON.stringify({ auth_mode: 'chatgpt', tokens: { access_token: access, refresh_token: 'synthetic-refresh', account_id: 'synthetic-account' } }));
    const store: RouterStore = await RouterStore.open(directory, { encrypt: Buffer.from, decrypt: (value: Buffer): string => value.toString() });
    expect(store.connections()).toEqual([{ ...original, method: 'codex_http' }]);
    const saved: { connections: unknown[] } = JSON.parse(await readFile(path.join(directory, 'settings.json'), 'utf8'));
    expect(saved.connections).toEqual(store.connections());
    const received: { url: string; payload: unknown; headers: Headers }[] = [];
    const adapter: CodexHttpAdapter = new CodexHttpAdapter(async (url, init): Promise<Response> => {
      received.push({ url: String(url), payload: JSON.parse(String(init?.body)), headers: new Headers(init?.headers) });
      return new Response(`data: ${JSON.stringify({ type: 'response.completed', response: { status: 'completed', output: [{ type: 'message', content: [{ type: 'output_text', text: 'Hello!' }] }], usage: { input_tokens: 8, output_tokens: 2 } } })}\n\n`);
    });
    listener = await startRouter(store, { codex_cli: adapter, codex_http: adapter, openrouter_http: adapter }, { port: 0, host: '127.0.0.1', require_api_key: false, browser_origins: ['https://antikythera.example'] }, '0.1.0');
    const origin: Record<string, string> = { Origin: 'https://antikythera.example' };
    const discovery: Response = await fetch(`${listener.endpoint}/v1/providers`, { headers: origin });
    const discovered: unknown = await discovery.json();
    expect(discovered).toMatchObject({ providers: [{ id, method: 'codex_http', models: [model] }] });
    const response: Response = await fetch(`${listener.endpoint}/v1/chat/completions`, { method: 'POST', headers: { ...origin, 'Content-Type': 'application/json' }, body: JSON.stringify({ model: `${id}/${model.id}`, reasoning_effort: 'low', messages: [{ role: 'user', content: 'Say hello.' }] }) });
    expect(response.status).toBe(200);
    expect(response.headers.get('Access-Control-Allow-Origin')).toBe(origin.Origin);
    const answer: unknown = await response.json();
    expect(answer).toMatchObject({ choices: [{ message: { content: 'Hello!' } }], usage: { prompt_tokens: 8, completion_tokens: 2, total_tokens: 10 }, provider_router: { connection_id: id, method: 'codex_http' } });
    expect(received).toHaveLength(1);
    expect(received[0]!.url).toBe('https://chatgpt.com/backend-api/codex/responses');
    expect(received[0]!.payload).toEqual({ model: model.id, instructions: '', input: [{ role: 'user', content: [{ type: 'input_text', text: 'Say hello.' }] }], tools: [], store: false, stream: true, reasoning: { effort: 'low' } });
    expect(received[0]!.headers.get('Authorization')).toBe(`Bearer ${access}`);
    expect(received[0]!.headers.get('ChatGPT-Account-ID')).toBe('synthetic-account');
    expect(JSON.stringify(answer)).not.toContain(access);
  } finally {
    await listener?.close();
    await rm(directory, { recursive: true, force: true });
  }
});
