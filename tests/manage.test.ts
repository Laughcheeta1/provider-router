/** Settings workflows preserve validation and persisted state when listener startup fails. */
import { mkdtemp, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { expect, it } from 'vitest';
import { RouterStore } from '../src/app/products/router/repository/settings/file.ts';
import { ManageUseCase, type SettingsRuntime } from '../src/app/products/router/use-cases/manage/use-case.ts';
import type { SettingsReply } from '../src/app/products/router/schemas/settings-contract.ts';
import type { ProviderService } from '../src/app/services/provider-service/service.ts';
// RevExt: 1
it('rejects insecure settings before persistence and retains valid settings after a failed restart', async (): Promise<void> => {
  const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-manage-'));
  try {  // RevExt: 3
    const store: RouterStore = await RouterStore.open(directory, { encrypt: Buffer.from, decrypt: (value: Buffer): string => value.toString() });  // RevExt: 5
    const providers: ProviderService = { infer: async (): Promise<never> => { throw new Error('unused'); }, models: async () => [] };  // RevExt: 7
    const runtime: SettingsRuntime = { endpoint: () => null, error: () => null, distributions: async () => [], discover: async () => [], platform: 'linux', revokeClient: (id: string): Promise<void> => store.revokeClient(id), restart: async (): Promise<never> => { throw new Error('port unavailable'); }, checkUpdates: async () => {} };
    const useCase: ManageUseCase = new ManageUseCase(store, providers, runtime);
    const original: ReturnType<RouterStore['publicState']>['network'] = store.publicState().network;
    for (const invalid of ['*', 'null', 'https://example.com/path', 'https://example.com/', 'http://remote.example']) {
      await expect(store.saveNetwork({ ...original, browser_origins: [invalid] })).rejects.toThrow();
      expect(store.publicState().network).toEqual(original);
    }
    await store.saveNetwork({ ...original, browser_origins: ['https://app.example', 'http://127.0.0.1:5173'] });
    expect(store.publicState().network.browser_origins).toEqual(['https://app.example', 'http://127.0.0.1:5173']);
    await store.saveNetwork(original);
    await expect(useCase.execute({ type: 'save_network', network: { ...original, host: '0.0.0.0' } })).rejects.toThrow('only supports loopback');
    expect(store.publicState().network).toEqual(original);
    await expect(useCase.execute({ type: 'save_network', network: { ...original, port: 47849 } })).rejects.toThrow('port unavailable');
    expect(store.publicState().network.port).toBe(47849);
    expect((await RouterStore.open(directory, { encrypt: Buffer.from, decrypt: (value: Buffer): string => value.toString() })).publicState().network.port).toBe(47849);
  } finally { await rm(directory, { recursive: true, force: true }); }  // RevExt: 9
});  // RevExt: 11
// RevExt: 2
it('returns discovery in settings without persisting or exposing a detected provider', async (): Promise<void> => {
  const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-discovery-'));
  try {  // RevExt: 4
    const store: RouterStore = await RouterStore.open(directory, { encrypt: Buffer.from, decrypt: (value: Buffer): string => value.toString() });  // RevExt: 6
    const providers: ProviderService = { infer: async (): Promise<never> => { throw new Error('unused'); }, models: async () => [] };  // RevExt: 8
    const runtime: SettingsRuntime = {
      endpoint: () => null, error: () => null, distributions: async () => ['Ubuntu'], platform: 'win32',
      discover: async (distributions: readonly string[]) => distributions.map((distribution: string) => ({ connection: { id: '019a6b00-0000-7000-8000-000000000001', name: 'Codex', method: 'codex_http' as const, environment: 'wsl' as const, wsl_distribution: distribution, exposed: false, models: [] }, status: 'ready' as const, detail: 'Existing Codex login detected.' })),
      revokeClient: async () => {}, restart: async () => {}, checkUpdates: async () => {},
    };
    const reply: SettingsReply = await new ManageUseCase(store, providers, runtime).execute({ type: 'snapshot' });
    expect(reply).toMatchObject({ ok: true, value: { detected_providers: [{ status: 'ready', connection: { wsl_distribution: 'Ubuntu', exposed: false } }], connections: [] } });
    expect(store.publicState().connections).toEqual([]);
    expect((await RouterStore.open(directory, { encrypt: Buffer.from, decrypt: (value: Buffer): string => value.toString() })).publicState().connections).toEqual([]);
  } finally { await rm(directory, { recursive: true, force: true }); }  // RevExt: 10
});  // RevExt: 12
