/** Interactive provider, client, and listener configuration using the shared settings bridge. */
import type { Connection, DetectedProvider, Model } from '../../../../common/schemas/contracts.ts';
import { uuid7 } from '../../../../common/identity/identity.ts';
import type { RouterStatus, SettingsBridge, SettingsCommand, SettingsReply, SettingsSnapshot } from '../../schemas/settings-contract.ts';
import type { StartupMode } from '../../schemas/state.ts';
import type { TerminalScreen } from './screen.ts';

export interface TerminalRouterActions {
  start(): Promise<void>;
  stop(): Promise<void>;
}

const startupOptions: readonly { label: string; value: StartupMode }[] = [
  { label: 'Run on startup', value: 'startup' },
  { label: 'Keep running after open', value: 'keep-after-open' },
  { label: 'Run only while open', value: 'while-open' },
];

export async function showTerminalSettings(screen: TerminalScreen, bridge: SettingsBridge, router: TerminalRouterActions): Promise<void> {
  const settings: TerminalSettings = new TerminalSettings(screen, bridge);
  for (;;) {
    try {
      const status: RouterStatus = await settings.status();
      const action: string | undefined = await screen.menu('Settings', [
        { label: 'Providers', value: 'providers', key: 'p', description: 'Connect Codex or OpenRouter and choose the models available to your clients.' },
        { label: 'API access', value: 'network', key: 'a', description: 'Set the listener address, client key policy, and approved website origins.' },
        { label: 'Client keys', value: 'clients', key: 'k', description: 'Create a key for an application or revoke access.' },
        { label: 'Startup behavior', value: 'startup', key: 's', description: 'Choose when the router starts and how long it stays running.' },
        { label: status.endpoint ? 'Stop API' : 'Start API', value: 'service', key: 'r', description: status.endpoint ? 'Stop serving requests from this router.' : 'Start serving requests using your saved settings.' },
        { label: 'Exit', value: 'exit', key: 'q', description: 'Close the terminal app.' },
      ], {
        dashboard: {
          endpoint: status.endpoint,
          error: status.error,
          startup: startupOptions.find((option): boolean => option.value === status.startup_mode)!.label,
          providers: status.connections.length,
          exposed: status.connections.filter((connection: Connection): boolean => connection.exposed).length,
        },
      });
      if (!action || action === 'exit') return;
      if (action === 'providers') await settings.providers(status);
      if (action === 'network') await settings.network(status);
      if (action === 'clients') await settings.clients(status);
      if (action === 'startup') await settings.startup(status);
      if (action === 'service') {
        if (status.endpoint) await router.stop();
        else await router.start();
      }
    } catch (error: unknown) {
      await screen.message('Settings error', [error instanceof Error ? error.message : 'The settings operation failed.']);
    }
  }
}

class TerminalSettings {
  constructor(private readonly screen: TerminalScreen, private readonly bridge: SettingsBridge) {}

  async clients(initial: RouterStatus): Promise<void> {
    let status: RouterStatus = initial;
    for (;;) {
      const action: string | undefined = await this.screen.menu('Client keys', [
        { label: 'Create a client key', value: 'create' },
        ...status.clients.map((client): { label: string; value: string } => ({ label: `${client.name}  (${client.id})`, value: client.id })),
      ]);
      if (!action) return;
      if (action === 'create') {
        const name: string | undefined = await this.screen.ask('Client name');
        if (!name?.trim()) continue;
        const client: { id: string; token: string } = await this.invoke({ type: 'create_client', name }) as { id: string; token: string };
        await this.screen.message('New client key', ['Copy this key now. It is shown once.', client.token]);
      } else {
        const client: RouterStatus['clients'][number] = status.clients.find((item): boolean => item.id === action)!;
        if (await this.confirm(`Revoke ${client.name}?`)) await this.invoke({ type: 'revoke_client', id: client.id });
      }

      status = await this.status();
    }
  }

  async network(status: RouterStatus): Promise<void> {
    const host: string | undefined = await this.screen.ask('Listener host', { initial: status.network.host });
    if (host === undefined) return;
    const port: string | undefined = await this.screen.ask('Listener port', { initial: String(status.network.port) });
    if (port === undefined) return;
    const requireKey: boolean | undefined = await this.toggle('Require client API keys', status.network.require_api_key);
    if (requireKey === undefined) return;
    const origins: string | undefined = await this.screen.ask('Approved website origins', { initial: (status.network.browser_origins ?? []).join(', '), hint: 'Enter exact origins separated by commas.' });
    if (origins === undefined) return;
    await this.invoke({ type: 'save_network', network: {
      host: host.trim(),
      port: Number(port),
      require_api_key: requireKey,
      browser_origins: origins.split(',').map((origin: string): string => origin.trim()).filter(Boolean),
    } });
  }

  async providers(initial: RouterStatus): Promise<void> {
    let status: RouterStatus = initial;
    for (;;) {
      const action: string | undefined = await this.screen.menu('Providers', [
        { label: 'Discover Codex installations', value: 'discover' },
        { label: 'Add a provider', value: 'add' },
        ...status.connections.map((connection: Connection): { label: string; value: string } => ({
          label: `${connection.name}  ·  ${connection.exposed ? 'Enabled' : 'Disabled'}  ·  ${connection.models.length} selected models`, value: connection.id,
        })),
      ]);
      if (!action) return;
      if (action === 'discover') await this.discover(status.connections);
      else if (action === 'add') await this.addProvider();
      else await this.editProvider(status.connections.find((connection: Connection): boolean => connection.id === action)!);
      status = await this.status();
    }
  }

  async startup(status: RouterStatus): Promise<void> {
    const current: string = startupOptions.find((option): boolean => option.value === status.startup_mode)!.label;
    const details: string[] = [`Current: ${current}`];
    if (process.platform === 'linux') details.push('Enable systemd user lingering for startup before sign-in.');
    const mode: StartupMode | undefined = await this.screen.menu('Startup behavior', startupOptions, { detail: details, initial: status.startup_mode });
    if (mode) await this.invoke({ type: 'save_startup', mode });
  }

  async status(): Promise<RouterStatus> {
    return await this.invoke({ type: 'status' }) as RouterStatus;
  }

  private async addProvider(): Promise<void> {
    const method: Connection['method'] | undefined = await this.screen.menu<Connection['method']>('Add a provider', [
      { label: 'Connect Codex', value: 'codex_http' },
      { label: 'Connect OpenRouter', value: 'openrouter_http' },
    ]);
    if (!method) return;
    const connection: Connection = { id: uuid7(), method, name: method === 'openrouter_http' ? 'OpenRouter' : 'Codex', exposed: false, models: [], environment: 'native' };
    if (method === 'codex_http' && process.platform === 'win32') {
      const snapshot: SettingsSnapshot = await this.invoke({ type: 'snapshot' }) as SettingsSnapshot;
      const distribution: string | undefined = await this.screen.menu('Codex environment', [
        { label: 'Windows', value: '' },
        ...snapshot.wsl_distributions.map((value: string): { label: string; value: string } => ({ label: `WSL · ${value}`, value })),
      ]);
      if (distribution === undefined) return;
      if (distribution) { connection.environment = 'wsl'; connection.wsl_distribution = distribution; }
    }

    const saved: Connection | undefined = await this.connectionDetails(connection, true);
    if (saved) await this.editProvider(saved);
  }

  private async confirm(title: string): Promise<boolean> {
    return (await this.screen.menu(title, [{ label: 'Cancel', value: false }, { label: 'Confirm', value: true }])) === true;
  }

  private async connectionDetails(connection: Connection, adding: boolean): Promise<Connection | undefined> {
    const name: string | undefined = await this.screen.ask('Provider name', { initial: connection.name });
    if (name === undefined) return undefined;
    const updated: Connection = { ...connection, name: name.trim() };
    let apiKey: string | undefined;
    if (connection.method === 'openrouter_http') {
      apiKey = await this.screen.ask('OpenRouter API key', { secret: true, hint: adding ? 'Paste the key to save it in the credential vault.' : 'Leave empty to keep the saved key.' });
      if (apiKey === undefined) return undefined;
      if (adding && !apiKey.trim()) throw new Error('Enter an OpenRouter API key.');
    } else {
      const home: string | undefined = await this.screen.ask('Codex home', { initial: connection.codex_home ?? '', hint: 'Leave empty to use the default Codex login.' });
      if (home === undefined) return undefined;
      updated.codex_home = home.trim() || undefined;
    }

    await this.invoke({ type: 'save_connection', connection: updated, api_key: apiKey?.trim() || undefined });
    return updated;
  }

  private async discover(saved: readonly Connection[]): Promise<void> {
    this.screen.progress('Discovering Codex installations…');
    const snapshot: SettingsSnapshot = await this.invoke({ type: 'snapshot' }) as SettingsSnapshot;
    const candidates: DetectedProvider[] = snapshot.detected_providers.filter((item: DetectedProvider): boolean =>
      !saved.some((connection: Connection): boolean => connection.method === item.connection.method &&
        (connection.environment ?? 'native') === (item.connection.environment ?? 'native') &&
        connection.wsl_distribution === item.connection.wsl_distribution && !connection.codex_home && !connection.executable));
    if (candidates.length === 0) { await this.screen.message('Discover Codex', ['Detected installations are already configured.']); return; }
    const choice: DetectedProvider | undefined = await this.screen.menu('Discover Codex', candidates.map((item: DetectedProvider) =>
      ({ label: `${item.connection.name}  ·  ${item.detail}`, value: item })));
    if (!choice || choice.status !== 'ready') return;
    await this.invoke({ type: 'save_connection', connection: choice.connection });
    await this.editProvider(choice.connection);
  }

  private async editProvider(initial: Connection): Promise<void> {
    let connection: Connection = initial;
    for (;;) {
      const current: Connection = connection;
      const action: string | undefined = await this.screen.menu(current.name, [
        { label: 'Configure connection', value: 'edit' },
        { label: 'Choose exposed models', value: 'models' },
        { label: current.exposed ? 'Disable provider' : 'Enable provider', value: 'expose' },
        { label: 'Remove provider', value: 'remove' },
      ]);
      if (!action) return;
      if (action === 'edit') connection = await this.connectionDetails(current, false) ?? current;
      if (action === 'models') connection = await this.models(current);
      if (action === 'expose') {
        connection = { ...current, exposed: !current.exposed };
        await this.invoke({ type: 'save_connection', connection });
      }
      if (action === 'remove' && await this.confirm(`Remove ${current.name}?`)) {
        await this.invoke({ type: 'remove_connection', id: current.id });
        return;
      }
    }
  }

  private async invoke(command: SettingsCommand): Promise<Extract<SettingsReply, { ok: true }>['value']> {
    const reply: SettingsReply = await this.bridge.invoke(command);
    if (!reply.ok) throw new Error(reply.error);
    return reply.value;
  }

  private async models(connection: Connection): Promise<Connection> {
    this.screen.progress(`Loading models for ${connection.name}…`);
    const available: Model[] = await this.invoke({ type: 'refresh_models', id: connection.id }) as Model[];
    if (!available.length) { await this.screen.message('Models', ['The provider returned no models.']); return connection; }
    const selected: Map<string, Model> = new Map(connection.models.map((model: Model): [string, Model] => [model.id, model]));
    let filter: string = '';
    for (;;) {
      const options: Model[] = available.filter((model: Model): boolean => `${model.name} ${model.id}`.toLowerCase().includes(filter.toLowerCase()));
      const action: string | undefined = await this.screen.menu('Models', [
        { label: 'Save selection', value: 'save' },
        { label: 'Search models', value: 'search' },
        ...options.map((model: Model): { label: string; value: string } => ({ label: `[${selected.has(model.id) ? '✓' : ' '}] ${model.name}  ·  ${model.id}`, value: `model:${model.id}` })),
      ], { detail: [`${selected.size} selected${filter ? `  ·  Search: ${filter}` : ''}`] });
      if (!action) return connection;
      if (action === 'save') {
        const updated: Connection = { ...connection, models: [...selected.values()] };
        await this.invoke({ type: 'save_connection', connection: updated });
        return updated;
      }

      if (action === 'search') { filter = await this.screen.ask('Search models', { initial: filter }) ?? filter; continue; }
      const model: Model = available.find((item: Model): boolean => item.id === action.slice(6))!;
      const previous: Model | undefined = selected.get(model.id);
      const expose: boolean | undefined = await this.toggle(`Expose ${model.name}`, Boolean(previous));
      if (expose === false) selected.delete(model.id);
      if (expose) {
        const permitted: string[] | undefined = await this.efforts(model, previous);
        if (!permitted) continue;
        if (!model.reasoning_efforts.length || permitted.length) {
          const preferred: string | null = previous?.default_reasoning_effort ?? model.default_reasoning_effort;
          selected.set(model.id, { ...model, reasoning_efforts: permitted, default_reasoning_effort: preferred && permitted.includes(preferred) ? preferred : permitted[0] ?? null });
        } else {
          await this.screen.message('Thinking efforts', ['Choose at least one thinking effort to expose this model.']);
        }
      }
    }
  }

  private async efforts(model: Model, previous?: Model): Promise<string[] | undefined> {
    const permitted: string[] = [];
    for (const effort of model.reasoning_efforts) {
      const allowed: boolean | undefined = await this.toggle(`Allow ${effort} thinking effort`, previous?.reasoning_efforts.includes(effort) ?? true);
      if (allowed === undefined) return undefined;
      if (allowed) permitted.push(effort);
    }

    return permitted;
  }

  private async toggle(title: string, current: boolean): Promise<boolean | undefined> {
    return this.screen.menu(title, [
      { label: `Yes${current ? ' (current)' : ''}`, value: true },
      { label: `No${current ? '' : ' (current)'}`, value: false },
    ], { initial: current });
  }
}
