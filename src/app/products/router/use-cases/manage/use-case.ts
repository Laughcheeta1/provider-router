/** Manage local connections, client grants and network settings through injected ports. */
import { RouterError, type Connection, type DetectedProvider, type Model } from '../../../../common/schemas/contracts.ts';
import type { ProviderService } from '../../../../services/provider-service/service.ts';
import type { RouterRepository } from '../../repository/settings/contract.ts';
import { validateNetwork } from '../../network-policy.ts';
import type { SettingsCommand, SettingsReply, SettingsSnapshot } from '../../schemas/settings-contract.ts';
export interface SettingsRuntime {
  endpoint(): string | null;
  error(): string | null;
  distributions(): Promise<string[]>;
  discover(distributions: readonly string[]): Promise<DetectedProvider[]>;
  platform: NodeJS.Platform;
  revokeClient(id: string): Promise<void>;
  restart(): Promise<void>;
  checkUpdates(): Promise<void>;
}
export class ManageUseCase {
  constructor(private readonly repository: RouterRepository, private readonly providers: ProviderService, private readonly runtime: SettingsRuntime) {}
  async execute(command: SettingsCommand): Promise<SettingsReply> {
    if (command.type === 'snapshot') {
      const distributions: string[] = await this.runtime.distributions();
      const detected: DetectedProvider[] = await this.runtime.discover(distributions);
      return { ok: true, value: { ...this.repository.publicState(), endpoint: this.runtime.endpoint() || null, error: this.runtime.error(), wsl_distributions: distributions, platform: this.runtime.platform, detected_providers: detected } satisfies SettingsSnapshot };
    }  // RevExt: 1
    if (command.type === 'save_connection') { await this.repository.saveConnection(command.connection, command.api_key); return { ok: true, value: null }; }
    if (command.type === 'remove_connection') { await this.repository.removeConnection(command.id); return { ok: true, value: null }; }
    if (command.type === 'refresh_models') {
      const connection: Connection | undefined = this.repository.connection(command.id);
      if (!connection) throw new RouterError('CONNECTION_UNAVAILABLE', 'This connection no longer exists.', 404);
      const models: Model[] = await this.providers.models(connection, AbortSignal.timeout(30_000));
      return { ok: true, value: models };
    }  // RevExt: 2
    if (command.type === 'create_client') return { ok: true, value: await this.repository.createClient(command.name) };
    if (command.type === 'revoke_client') { await this.runtime.revokeClient(command.id); return { ok: true, value: null }; }
    if (command.type === 'save_network') { validateNetwork(command.network); await this.repository.saveNetwork(command.network); await this.runtime.restart(); return { ok: true, value: null }; }
    await this.runtime.checkUpdates();
    return { ok: true, value: null };
  }
}
