/** Discovery reports local readiness without enabling connections or disclosing credentials. */
import { expect, it } from 'vitest';
import { LocalProviderDiscovery, type DiscoveryPorts } from '../src/app/services/provider-service/providers/discovery.ts';
import type { DetectedProvider } from '../src/app/common/schemas/contracts.ts';

function ports(installed: boolean, loggedIn: boolean): DiscoveryPorts {
  return {
    native: async () => ({ executable: 'codex', prefix: [], environment: {} }),
    wsl: async (distribution: string) => ({ distribution, codexHome: '/home/user/.codex', catalogDirectory: '/unused', executable: '/usr/bin/codex', searchPath: '/usr/bin' }),
    succeeds: async (_executable: string, args: readonly string[]): Promise<boolean> => args.at(-1) === '--version' ? installed : loggedIn,
  };
}

it('discovers native and WSL logins with stable disabled candidate identities', async (): Promise<void> => {
  const discovery: LocalProviderDiscovery = new LocalProviderDiscovery(ports(true, true));
  const first: DetectedProvider[] = await discovery.discover(['Ubuntu']);
  const second: DetectedProvider[] = await discovery.discover(['Ubuntu']);
  expect(first.map((provider: DetectedProvider): string => provider.status)).toEqual(['ready', 'ready']);
  expect(first.map((provider: DetectedProvider): string => provider.connection.id)).toEqual(second.map((provider: DetectedProvider): string => provider.connection.id));
  expect(first[0]!.connection).toMatchObject({ method: 'codex_http', environment: 'native', exposed: false, models: [] });
  expect(first[1]!.connection).toMatchObject({ method: 'codex_http', environment: 'wsl', wsl_distribution: 'Ubuntu', exposed: false, models: [] });
  expect(first[0]!.connection.id[14]).toBe('7');
});

it('distinguishes a missing CLI from an installation that needs login', async (): Promise<void> => {
  expect((await new LocalProviderDiscovery(ports(false, false)).discover([]))[0]!.status).toBe('not_installed');
  expect((await new LocalProviderDiscovery(ports(true, false)).discover([]))[0]!.status).toBe('login_required');
});

it('contains one failed environment without hiding other ready providers or exposing its error', async (): Promise<void> => {
  const dependencies: DiscoveryPorts = { ...ports(true, true), wsl: async (): Promise<never> => { throw new Error('secret-account-details'); } };
  const results: DetectedProvider[] = await new LocalProviderDiscovery(dependencies).discover(['Broken']);
  expect(results[0]!.status).toBe('ready');
  expect(results[1]!.status).toBe('not_installed');
  expect(JSON.stringify(results)).not.toContain('secret-account-details');
});
