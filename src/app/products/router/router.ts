/** Authenticated, bounded local inference API. */
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { ZodError } from 'zod';
import { inferenceSchema, PROTOCOL_VERSION, RouterError, type Connection, type InferenceRequest, type InferenceResult, type Method, type ProviderAdapter, type ProviderDescriptor } from '../../common/schemas/contracts.ts';
import type { RouterRepository } from './repository/settings/contract.ts';
import type { Client, NetworkSettings } from './schemas/state.ts';
import { buildInferenceUseCase } from './dependencies.ts';
import { validateNetwork } from './network-policy.ts';
export { validateNetwork } from './network-policy.ts';
import type { InferenceUseCase } from './use-cases/inference/use-case.ts';
import { decodeOpenAI, modelList, validateOpenAIResult, writeOpenAI, type OpenAIRequest } from './presentation/openai-api.ts';
// RevExt: 1
export interface ListeningRouter { endpoint: string; close(): Promise<void>; revokeClient(id: string): Promise<void> }
// RevExt: 2
/** Start one independent service; no client connection owns the service lifecycle. */
export async function startRouter(store: RouterRepository, adapters: Record<Method, ProviderAdapter>, network: NetworkSettings, version: string): Promise<ListeningRouter> {
  validateNetwork(network);
  const inference: InferenceUseCase = buildInferenceUseCase(store, adapters);
  const health: Record<string, unknown> = { name: 'provider-router', protocol_version: PROTOCOL_VERSION, instance_id: store.publicState().instance_id, version, authentication_required: network.require_api_key };
  const handle: (request: IncomingMessage, response: ServerResponse) => void = (request: IncomingMessage, response: ServerResponse): void => {
    void dispatch(request, response).catch((error: unknown): void => {
      if (response.destroyed || response.writableEnded) return;
      const failure: RouterError = error instanceof RouterError ? error : error instanceof ZodError ? new RouterError('INVALID_REQUEST', 'The request does not match the API contract.') : new RouterError('PROVIDER_FAILED', 'The provider could not complete this request.', 502);
      json(response, failure.status, { error: { code: failure.code, message: failure.message, type: failure.status === 401 ? 'authentication_error' : failure.status === 429 ? 'rate_limit_error' : failure.status >= 500 ? 'api_error' : 'invalid_request_error', param: null } });
    });
  };
  const server: Server = createServer(handle);
  server.headersTimeout = 10_000; server.requestTimeout = 30_000;
  await new Promise<void>((resolve, reject): void => { server.once('error', reject); server.listen(network.port, network.host, (): void => { server.removeListener('error', reject); resolve(); }); });
  const address: AddressInfo = server.address() as AddressInfo;
  const host: string = address.family === 'IPv6' ? `[${address.address}]` : address.address;
  const endpoint: string = `http://${host}:${address.port}`;
  return {
    endpoint,
    close: async (): Promise<void> => { inference.dispose(); await new Promise<void>((resolve, reject): void => { server.close((error?: Error): void => error ? reject(error) : resolve()); server.closeAllConnections(); }); },
    revokeClient: async (id: string): Promise<void> => { await inference.revokeClient(id); },
  };
// RevExt: 3
  async function dispatch(request: IncomingMessage, response: ServerResponse): Promise<void> {
    if (request.headers.host && ![new URL(endpoint).host, `localhost:${new URL(endpoint).port}`].includes(request.headers.host)) throw new RouterError('HOST_REJECTED', 'Use the configured local Router endpoint.', 403);
    const origin: string | undefined = request.headers.origin;
    if (origin) {
      if (!network.browser_origins?.includes(origin)) throw new RouterError('BROWSER_ORIGIN_REJECTED', 'Approve this website origin in Provider Router API access settings.', 403);
      response.setHeader('Access-Control-Allow-Origin', origin);
      response.setHeader('Vary', 'Origin');
      response.setHeader('Access-Control-Expose-Headers', 'x-request-id');
    }  // RevExt: 9
    if (request.method === 'OPTIONS') {
      const method: string = request.headers['access-control-request-method'] || '';
      const headers: string[] = (request.headers['access-control-request-headers'] || '').toLowerCase().split(',').map((header: string): string => header.trim()).filter(Boolean);
      if (!origin || !['GET', 'POST'].includes(method) || headers.some((header: string): boolean => !['authorization', 'content-type', 'x-request-id'].includes(header))) throw new RouterError('CORS_PREFLIGHT_REJECTED', 'This browser request method or header is not supported.', 403);
      response.writeHead(204, { 'Access-Control-Allow-Methods': 'GET, POST', 'Access-Control-Allow-Headers': 'authorization, content-type, x-request-id', 'Cache-Control': 'no-store' });
      response.end(); return;
    }  // RevExt: 10
    const target: URL = new URL(request.url || '/', endpoint);
    if (target.pathname === '/health' && request.method === 'GET') { json(response, 200, health); return; }
    const token: string | undefined = /^Bearer ([A-Za-z0-9_-]{43})$/.exec(request.headers.authorization || '')?.[1];
    const client: Client | undefined = token ? store.authenticate(token) : undefined;
    if (network.require_api_key && !client) throw new RouterError('CLIENT_UNAUTHORIZED', 'A valid Provider Router client key is required.', 401);
    if (target.pathname === '/v1/connectivity' && request.method === 'GET') {
      const connections: Connection[] = store.connections().filter((connection: Connection): boolean => connection.exposed);
      const models: number = connections.reduce((count: number, connection: Connection): number => count + connection.models.length, 0);
      json(response, 200, { ...health, ready: models > 0, providers: connections.length, models }); return;
    }  // RevExt: 11
    if (target.pathname === '/v1/models' && request.method === 'GET') { json(response, 200, modelList(store.connections())); return; }
    if (target.pathname === '/v1/providers' && request.method === 'GET') {
      const providers: ProviderDescriptor[] = store.connections().filter((connection: Connection): boolean => connection.exposed).map((connection: Connection): ProviderDescriptor => ({ id: connection.id, name: connection.name, provider: connection.method === 'openrouter_http' ? 'openrouter' : 'codex', method: connection.method, models: connection.models }));
      json(response, 200, { protocol_version: PROTOCOL_VERSION, providers }); return;
    }  // RevExt: 12
    const cancel: RegExpExecArray | null = /^\/v1\/inference\/([0-9a-f-]+)\/cancel$/.exec(target.pathname);
    if (cancel && request.method === 'POST') {
      inference.cancel(client?.id ?? store.publicState().instance_id, cancel[1]!); json(response, 200, { cancelled: true }); return;
    }  // RevExt: 13
    if (!['/v1/inference', '/v1/chat/completions', '/v1/responses'].includes(target.pathname) || request.method !== 'POST') throw new RouterError('NOT_FOUND', 'This endpoint is unavailable.', 404);
    if (!request.headers['content-type']?.startsWith('application/json')) throw new RouterError('CONTENT_TYPE_REQUIRED', 'Use application/json for inference requests.', 415);
    const value: unknown = await readJson(request);
    const correlation: string | string[] | undefined = request.headers['x-request-id'];
    const canonical: OpenAIRequest | null = target.pathname === '/v1/inference' ? null : decodeOpenAI(target.pathname === '/v1/responses' ? 'responses' : 'chat', value, store.connections(), typeof correlation === 'string' ? correlation : undefined);
    const input: InferenceRequest = canonical?.inference || inferenceSchema.parse(value);
    response.setHeader('x-request-id', input.request_id);
    const abort: AbortController = new AbortController();
    const disconnect: () => void = (): void => { if (!response.writableEnded) abort.abort(); };
    response.once('close', disconnect);
    request.once('aborted', disconnect);
    try {
      const result: InferenceResult = await inference.execute(client?.id ?? store.publicState().instance_id, client ? token! : null, input, abort);
      if (canonical) { validateOpenAIResult(canonical, result); writeOpenAI(response, canonical, result); }
      else json(response, 200, result);
    } catch (error: unknown) {
      if (abort.signal.aborted) throw new RouterError('CANCELLED', 'The inference request was cancelled.', 499);
      throw error;
    } finally {
      response.removeListener('close', disconnect); request.removeListener('aborted', disconnect);
    }  // RevExt: 14
  }
}  // RevExt: 6
// RevExt: 4
async function readJson(request: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  let bytes: number = 0;
  for await (const chunk of request) {
    const buffer: Buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk as string);
    bytes += buffer.length;
    if (bytes > 16_777_216) throw new RouterError('REQUEST_TOO_LARGE', 'The request exceeds the supported size.', 413);
    chunks.push(buffer);
  }
  try { return JSON.parse(Buffer.concat(chunks).toString()) as unknown; }
  catch { throw new RouterError('INVALID_JSON', 'The request body must contain valid JSON.'); }
}  // RevExt: 7
// RevExt: 5
function json(response: ServerResponse, status: number, body: unknown): void {
  response.writeHead(status, { 'Content-Type': 'application/json', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff' });
  response.end(JSON.stringify(body));
}  // RevExt: 8
