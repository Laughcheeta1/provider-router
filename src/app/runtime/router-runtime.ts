/** Own the settings workflows and inference listener independently of desktop and terminal UI. */
import { rename, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { RouterError, type Method, type ProviderAdapter } from '../common/schemas/contracts.ts';
import { uuid7 } from '../common/identity/identity.ts';
import { buildManageUseCase } from '../products/router/dependencies.ts';
import { RouterStore } from '../products/router/repository/settings/file.ts';
import { startRouter, type ListeningRouter } from '../products/router/router.ts';
import type { SettingsCommand, SettingsReply } from '../products/router/schemas/settings-contract.ts';
import type { StartupMode } from '../products/router/schemas/state.ts';
import type { ManageUseCase } from '../products/router/use-cases/manage/use-case.ts';
import { createLocalProviderDiscovery, type LocalProviderDiscovery } from '../services/provider-service/providers/discovery.ts';
import { CodexHttpAdapter, OpenRouterAdapter } from '../services/provider-service/providers/http-providers.ts';
import { listWslDistributions } from '../services/provider-service/providers/wsl-codex.ts';
import { createCredentialVault } from './credential-vault.ts';
import type { LaunchCommand } from './launch.ts';
import type { RouterProfile } from './profile.ts';
import { configureStartup } from './startup.ts';

export class RouterRuntime {
  private readonly manage: ManageUseCase;
  private readonly adapters: Record<Method, ProviderAdapter>;
  private listener: ListeningRouter | null = null;
  private startupError: string | null = null;
  private operations: Promise<void> = Promise.resolve();

  private constructor(
    private readonly profile: RouterProfile,
    private readonly store: RouterStore,
    private readonly launch: LaunchCommand,
    private readonly version: string,
    private readonly listening: boolean,
  ) {
    const codex: CodexHttpAdapter = new CodexHttpAdapter();
    const discovery: LocalProviderDiscovery = createLocalProviderDiscovery();
    this.adapters = { codex_cli: codex, codex_http: codex, openrouter_http: new OpenRouterAdapter((id: string): string | undefined => store.secret(id)) };
    this.manage = buildManageUseCase(store, this.adapters, {
      endpoint: (): string | null => this.listener?.endpoint ?? null,
      error: (): string | null => this.startupError,
      distributions: listWslDistributions,
      platform: process.platform,
      discover: (distributions: readonly string[]) => discovery.discover(distributions),
      revokeClient: async (id: string): Promise<void> => {
        if (this.listener) await this.listener.revokeClient(id);
        else await store.revokeClient(id);
      },
      restart: (): Promise<void> => this.restart(),
      saveStartup: (mode: StartupMode): Promise<void> => this.saveStartup(mode),
      checkUpdates: async (): Promise<never> => { throw new RouterError('UPDATES_UNAVAILABLE', 'Check for updates in the desktop app or download a newer release.'); },
    });
  }

  static async open(profile: RouterProfile, launch: LaunchCommand, version: string, listening: boolean): Promise<RouterRuntime> {
    const store: RouterStore = await RouterStore.open(profile.directory, createCredentialVault(profile.directory), listening);
    return new RouterRuntime(profile, store, launch, version, listening);
  }

  async close(): Promise<void> {
    await this.operations;
    await this.closeListener();
  }

  execute(command: SettingsCommand): Promise<SettingsReply> {
    const result: Promise<SettingsReply> = this.operations.then(async (): Promise<SettingsReply> => {
      try {
        return await this.manage.execute(command);
      } catch (error: unknown) {
        return { ok: false, error: error instanceof RouterError ? error.message : 'The settings operation could not finish. Check the configuration and provider connection.' };
      }
    });
    this.operations = result.then((): void => {});
    return result;
  }

  async restart(): Promise<void> {
    if (!this.listening) return;
    await this.closeListener();
    let temporary: string | undefined;
    try {
      this.listener = await startRouter(this.store, this.adapters, this.store.publicState().network, this.version);
      const descriptor: Record<string, unknown> = { endpoint: this.listener.endpoint, protocol_version: 1, instance_id: this.store.publicState().instance_id, pid: process.pid, version: this.version };
      temporary = path.join(this.profile.directory, `discovery-${uuid7()}.tmp`);
      await writeFile(temporary, JSON.stringify(descriptor), { mode: 0o600 });
      await rename(temporary, path.join(this.profile.directory, 'discovery.json'));
      this.startupError = null;
    } catch {
      await this.closeListener();
      this.startupError = 'The API could not start. Check the local listener host and port.';
    } finally {
      if (temporary) await rm(temporary, { force: true });
    }
  }

  startupMode(): StartupMode {
    return this.store.publicState().startup_mode;
  }

  private async closeListener(): Promise<void> {
    const listener: ListeningRouter | null = this.listener;
    this.listener = null;
    if (!this.listening) return;
    try { await listener?.close(); }
    finally { await rm(path.join(this.profile.directory, 'discovery.json'), { force: true }); }
  }

  private async saveStartup(mode: StartupMode): Promise<void> {
    const previous: StartupMode = this.startupMode();
    if (mode === previous && mode !== 'startup') return;
    const changesRegistration: boolean = mode === 'startup' || previous === 'startup';
    if (changesRegistration) await configureStartup(this.profile, this.launch, mode === 'startup');
    try {
      await this.store.saveStartupMode(mode);
    } catch (error: unknown) {
      if (changesRegistration) await configureStartup(this.profile, this.launch, previous === 'startup').catch((): void => {});
      throw error;
    }
  }
}
