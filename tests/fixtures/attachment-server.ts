/** Run the real Router with fake upstream HTTP responses for cross-application tests. */
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { uuid7 } from '../../src/app/common/identity/identity.ts';
import type { Connection, Model } from '../../src/app/common/schemas/contracts.ts';
import { RouterStore } from '../../src/app/products/router/repository/settings/file.ts';
import { startRouter } from '../../src/app/products/router/router.ts';
import { CodexHttpAdapter, OpenRouterAdapter } from '../../src/app/services/provider-service/providers/http-providers.ts';

const directory: string = process.argv[2]!;
const model: Model = { id: 'attachment-test', name: 'Fixture model', reasoning_efforts: ['low'], default_reasoning_effort: 'low',
  context_window: 400_000, max_output_tokens: 4000, input_modalities: ['text', 'image'], tools: true, structured_output: true };
const codex: Connection = { id: uuid7(), name: 'Codex test', method: 'codex_http', exposed: true, models: [model], codex_home: directory };
const openrouter: Connection = { id: uuid7(), name: 'OpenRouter test', method: 'openrouter_http', exposed: true, models: [model] };
const store: RouterStore = await RouterStore.open(directory, { encrypt: Buffer.from, decrypt: (value: Buffer): string => value.toString() });
await store.saveConnection(codex);
await store.saveConnection(openrouter);
const encoded: string = Buffer.from(JSON.stringify({ exp: Date.now() / 1000 + 3600 })).toString('base64url');
await writeFile(join(directory, 'auth.json'), JSON.stringify({ auth_mode: 'chatgpt', tokens: {
  access_token: `header.${encoded}.signature`, refresh_token: 'synthetic-refresh', account_id: 'synthetic-account',
} }));
const capture: string = join(directory, 'captured.json');
const codexAdapter: CodexHttpAdapter = new CodexHttpAdapter(async (_url, init): Promise<Response> => {
  await writeFile(capture, String(init?.body));
  return new Response(`data: ${JSON.stringify({ type: 'response.completed', response: { status: 'completed',
    output: [{ type: 'message', content: [{ type: 'output_text', text: 'Attachment received.' }] }],
    usage: { input_tokens: 500, output_tokens: 5 } } })}\n\n`);
});
const openrouterAdapter: OpenRouterAdapter = new OpenRouterAdapter((): string => 'synthetic-key', async (_url, init): Promise<Response> => {
  await writeFile(capture, String(init?.body));
  return Response.json({ choices: [{ message: { content: 'Attachment received.' }, finish_reason: 'stop' }],
    usage: { prompt_tokens: 500, completion_tokens: 5 } });
});
const listener = await startRouter(store, { codex_cli: codexAdapter, codex_http: codexAdapter, openrouter_http: openrouterAdapter },
  { host: '127.0.0.1', port: 0, require_api_key: false }, '0.1.0');
process.on('SIGTERM', (): void => { void listener.close(); });
process.stdout.write(JSON.stringify({ endpoint: listener.endpoint, codex: codex.id, openrouter: openrouter.id, model: model.id }) + '\n');
