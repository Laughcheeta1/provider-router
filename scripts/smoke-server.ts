/** Isolated headless development service for real protocol/provider rehearsals on WSL. */
import { mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { CodexHttpAdapter, OpenRouterAdapter } from '../src/app/services/provider-service/providers/http-providers.ts';
import { uuid7 } from '../src/app/common/identity/identity.ts';
import { startRouter, type ListeningRouter } from '../src/app/products/router/router.ts';
import { RouterStore, type SecretCipher } from '../src/app/products/router/repository/settings/file.ts';
import type { Connection } from '../src/app/common/schemas/contracts.ts';

async function main(): Promise<void> {
  const directory: string | undefined = process.env.PROVIDER_ROUTER_DATA_DIR;
  if (!directory || !path.isAbsolute(directory)) throw new Error('Set an absolute PROVIDER_ROUTER_DATA_DIR for an isolated rehearsal.');
  const noSecrets: SecretCipher = { encrypt: (): never => { throw new Error('Headless rehearsal does not store provider credentials.'); }, decrypt: (): never => { throw new Error('Headless rehearsal does not read provider credentials.'); } };
  const store: RouterStore = await RouterStore.open(directory, noSecrets);
  const adapter: CodexHttpAdapter = new CodexHttpAdapter();
  const method: 'codex_http' = 'codex_http';
  const connection: Connection = {
    id: uuid7(), name: 'Synthetic Codex rehearsal', method, exposed: true, models: [],
    ...(process.env.PROVIDER_ROUTER_CODEX_EXECUTABLE ? { executable: process.env.PROVIDER_ROUTER_CODEX_EXECUTABLE } : {}),
    ...(process.env.PROVIDER_ROUTER_CODEX_HOME ? { codex_home: process.env.PROVIDER_ROUTER_CODEX_HOME } : {}),
    ...(process.env.PROVIDER_ROUTER_SMOKE_ENVIRONMENT === 'wsl' ? { environment: 'wsl', wsl_distribution: process.env.PROVIDER_ROUTER_WSL_DISTRIBUTION } : {}),
  };
  connection.models = await adapter.models(connection, AbortSignal.timeout(10_000));
  await store.saveConnection(connection);
  const client: { id: string; token: string } = await store.createClient('Synthetic development rehearsal');
  const server: ListeningRouter = await startRouter(store, { codex_cli: adapter, codex_http: adapter, openrouter_http: new OpenRouterAdapter((): undefined => undefined) }, { port: Number(process.env.PROVIDER_ROUTER_PORT || 47831), host: '127.0.0.1', require_api_key: true }, '0.1.0');
  await mkdir(directory, { recursive: true, mode: 0o700 });
  await writeFile(path.join(directory, 'ready.json'), JSON.stringify({ endpoint: server.endpoint, token: client.token, client_id: client.id, connection_id: connection.id, instance_id: store.publicState().instance_id, protocol_version: 1 }), { mode: 0o600 });
  console.log(`Provider Router rehearsal listening at ${server.endpoint}`);
  const stop: () => void = (): void => { void server.close().then((): never => process.exit(0)); };
  process.once('SIGTERM', stop); process.once('SIGINT', stop);
}
void main().catch((): void => { console.error('Provider Router rehearsal could not start.'); process.exitCode = 1; });
