/** Settings workflows preserve validation and persisted state when listener startup fails. */
import { mkdtemp, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { expect, it } from 'vitest';
import { RouterStore } from '../src/app/products/router/repository/settings/file.ts';
import { ManageUseCase, type SettingsRuntime } from '../src/app/products/router/use-cases/manage/use-case.ts';
import type { ProviderService } from '../src/app/services/provider-service/service.ts';

it('rejects insecure settings before persistence and retains valid settings after a failed restart', async (): Promise<void> => {
  const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-manage-'));
  try {
    const store: RouterStore = await RouterStore.open(directory, { encrypt: Buffer.from, decrypt: (value: Buffer): string => value.toString() });
    const providers: ProviderService = { infer: async (): Promise<never> => { throw new Error('unused'); }, models: async () => [] };
    const runtime: SettingsRuntime = { endpoint: () => null, error: () => null, distributions: async () => [], platform: 'linux', revokeClient: (id: string): Promise<void> => store.revokeClient(id), restart: async (): Promise<never> => { throw new Error('port unavailable'); }, checkUpdates: async () => {} };
    const useCase: ManageUseCase = new ManageUseCase(store, providers, runtime);
    const original: ReturnType<RouterStore['publicState']>['network'] = store.publicState().network;
    await expect(useCase.execute({ type: 'save_network', network: { ...original, host: '0.0.0.0' } })).rejects.toThrow('Enable remote access');
    expect(store.publicState().network).toEqual(original);
    await expect(useCase.execute({ type: 'save_network', network: { ...original, port: 47849 } })).rejects.toThrow('port unavailable');
    expect(store.publicState().network.port).toBe(47849);
    expect((await RouterStore.open(directory, { encrypt: Buffer.from, decrypt: (value: Buffer): string => value.toString() })).publicState().network.port).toBe(47849);
  } finally { await rm(directory, { recursive: true, force: true }); }
});
