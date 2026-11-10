/** The official OpenAI SDK talks directly to the real HTTP service without router-specific fields. */
import OpenAI from 'openai';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { mkdtemp, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { uuid7 } from '../src/app/common/identity/identity.ts';
import { startRouter, type ListeningRouter } from '../src/app/products/router/router.ts';
import { RouterStore } from '../src/app/products/router/repository/settings/file.ts';
import type { Connection, InferenceRequest, InferenceResult, Model, ProviderAdapter } from '../src/app/common/schemas/contracts.ts';

let directory: string;
let listener: ListeningRouter;
let client: OpenAI;
let selected: string;
let store: RouterStore;
let lastRequest: InferenceRequest | null;
const model: Model = { id: 'example/model', name: 'Synthetic model', reasoning_efforts: ['low', 'high'], default_reasoning_effort: 'low', context_window: 32000, max_output_tokens: 4000, tools: true, structured_output: true, input_modalities: ['text'] };

beforeEach(async (): Promise<void> => {
  directory = await mkdtemp(path.join(os.tmpdir(), 'router-sdk-test-'));
  store = await RouterStore.open(directory, { encrypt: (value: string): Buffer => Buffer.from(value), decrypt: (value: Buffer): string => value.toString() });
  const connection: Connection = { id: uuid7(), name: 'Synthetic provider', method: 'codex_cli', exposed: true, models: [model] };
  await store.saveConnection(connection); selected = `${connection.id}/${model.id}`;
  const key: { id: string; token: string } = await store.createClient('Official SDK test');
  const adapter: ProviderAdapter = { models: async (): Promise<Model[]> => [model], infer: async (request: InferenceRequest): Promise<InferenceResult> => {
    lastRequest = request;
    return { request_id: request.request_id, model: request.model, content: request.tools.length ? '' : '{"answer":42}', tool_calls: request.tools.length ? [{ id: 'call-synthetic', type: 'function', function: { name: 'read_balance', arguments: '{"id":"a"}' } }] : [], usage: { input_tokens: 21, output_tokens: 8 }, provider: 'codex', method: 'codex_cli', reasoning_effort: request.reasoning_effort };
  } };
  listener = await startRouter(store, { codex_cli: adapter, codex_http: adapter, openrouter_http: adapter }, { host: '127.0.0.1', port: 0, remote_enabled: false, trusted_proxy: false }, '0.1.0');
  client = new OpenAI({ apiKey: key.token, baseURL: `${listener.endpoint}/v1`, maxRetries: 0 }); lastRequest = null;
});
afterEach(async (): Promise<void> => { await listener.close(); await rm(directory, { recursive: true, force: true }); });

describe('OpenAI SDK compatibility', (): void => {
  it('discovers models and completes Chat Completions with structured output and reasoning selection', async (): Promise<void> => {
    const models = await client.models.list(); expect(models.data[0]?.id).toBe(selected);
    const result = await client.chat.completions.create({ model: selected, messages: [{ role: 'user', content: 'Synthetic question' }], reasoning_effort: 'low', response_format: { type: 'json_schema', json_schema: { name: 'answer', schema: { type: 'object', properties: { answer: { type: 'integer' } }, required: ['answer'] } } } });
    expect(result.object).toBe('chat.completion'); expect(result.choices[0]?.message.content).toBe('{"answer":42}'); expect(result.model).toBe(selected); expect(result.usage?.total_tokens).toBe(29); expect(lastRequest?.reasoning_effort).toBe('low');
    expect(result._request_id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7/);
  });

  it('returns standard function tool calls and forwards complete history associations', async (): Promise<void> => {
    const result = await client.chat.completions.create({ model: selected, messages: [{ role: 'assistant', content: null, tool_calls: [{ id: 'old-call', type: 'function', function: { name: 'read_balance', arguments: '{"id":"b"}' } }] }, { role: 'tool', tool_call_id: 'old-call', content: '{"balance":1}' }], tools: [{ type: 'function', function: { name: 'read_balance', parameters: { type: 'object', properties: { id: { type: 'string' } }, required: ['id'] } } }], tool_choice: 'required' });
    expect(result.choices[0]?.finish_reason).toBe('tool_calls'); expect(result.choices[0]?.message.tool_calls?.[0]).toMatchObject({ id: 'call-synthetic', function: { name: 'read_balance' } }); expect(lastRequest?.messages[1]?.tool_call_id).toBe('old-call');
  });

  it('streams valid Chat Completion chunks and a final usage event', async (): Promise<void> => {
    const stream = await client.chat.completions.create({ model: selected, messages: [{ role: 'user', content: 'Synthetic question' }], stream: true, stream_options: { include_usage: true } });
    let content: string = ''; let finished: string | null = null; let usage: number = 0;
    for await (const chunk of stream) { content += chunk.choices[0]?.delta.content || ''; finished = chunk.choices[0]?.finish_reason || finished; if (chunk.usage) usage = chunk.usage.total_tokens; }
    expect(content).toBe('{"answer":42}'); expect(finished).toBe('stop'); expect(usage).toBe(29);
  });

  it('completes Responses with instructions, input, schema and native function result associations', async (): Promise<void> => {
    const result = await client.responses.create({ model: selected, instructions: 'Synthetic instruction', input: [{ role: 'user', content: [{ type: 'input_text', text: 'Synthetic question' }] }, { type: 'function_call', call_id: 'old-call', name: 'read_balance', arguments: '{"id":"a"}' }, { type: 'function_call_output', call_id: 'old-call', output: '{"balance":42}' }], reasoning: { effort: 'low' }, text: { format: { type: 'json_schema', name: 'answer', schema: { type: 'object', properties: { answer: { type: 'integer' } }, required: ['answer'] } } } });
    expect(result.object).toBe('response'); expect(result.status).toBe('completed'); expect(result.output_text).toBe('{"answer":42}'); expect(lastRequest?.messages[0]).toMatchObject({ role: 'system', content: 'Synthetic instruction' }); expect(lastRequest?.messages.at(-1)?.tool_call_id).toBe('old-call');
  });

  it('streams standard Responses text and completion events', async (): Promise<void> => {
    const stream = await client.responses.create({ model: selected, input: 'Synthetic question', stream: true });
    const events: string[] = []; let content: string = ''; let completed: boolean = false;
    for await (const event of stream) { events.push(event.type); if (event.type === 'response.output_text.delta') content += event.delta; if (event.type === 'response.completed') completed = event.response.status === 'completed'; }
    expect(events[0]).toBe('response.created'); expect(events).toContain('response.content_part.done'); expect(content).toBe('{"answer":42}'); expect(completed).toBe(true);
  });

  it('echoes the configured Responses tools while enforcing tool_choice none', async (): Promise<void> => {
    const result = await client.responses.create({ model: selected, input: 'Synthetic question', tool_choice: 'none', tools: [{ type: 'function', name: 'read_balance', parameters: { type: 'object' }, strict: false }] });
    expect(result.tool_choice).toBe('none'); expect(result.tools).toMatchObject([{ type: 'function', name: 'read_balance' }]); expect(lastRequest?.tools).toEqual([]);
  });

  it('rejects ambiguous models and unsupported SDK options with standard errors before inference', async (): Promise<void> => {
    await store.saveConnection({ id: uuid7(), name: 'Another account', method: 'codex_cli', exposed: true, models: [model] });
    await expect(client.chat.completions.create({ model: model.id, messages: [{ role: 'user', content: 'Synthetic question' }] })).rejects.toMatchObject({ status: 400, code: 'MODEL_AMBIGUOUS', type: 'invalid_request_error' });
    await expect(client.chat.completions.create({ model: selected, messages: [{ role: 'user', content: 'Synthetic question' }], temperature: 0.5 })).rejects.toMatchObject({ status: 400, code: 'INVALID_REQUEST', type: 'invalid_request_error' });
    expect(lastRequest).toBeNull();
  });
});
