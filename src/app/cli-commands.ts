/** Map scriptable commands to the same settings operations used by both interactive apps. */
import { parseNativeJson } from './common/schemas/native-json.ts';
import { parseArgs } from 'node:util';
import { z } from 'zod';
import { uuid7 } from './common/identity/identity.ts';
import { RouterError, type Connection, type DetectedProvider, type Model } from './common/schemas/contracts.ts';
import { commandSchema } from './products/router/schemas/settings-command.ts';
import type { RouterStatus, SettingsCommand, SettingsReply, SettingsSnapshot } from './products/router/schemas/settings-contract.ts';
import { networkSchema, startupModeSchema } from './products/router/schemas/state.ts';
import type { ControlClient } from './runtime/control-client.ts';

export const commandHelp: string = `Usage: provider-router [--data-dir DIRECTORY] [command]

With no command, open Provider Router in the terminal.

  serve                               Run the API in the foreground
  status                              Show the listener and current settings
  stop                                Stop the running API
  startup [startup|keep-after-open|while-open]
  providers list
  providers discover
  providers use <native|wsl-distribution>
  providers add <codex|openrouter> [--name NAME] [--home PATH] [--wsl NAME] [--key-stdin]
  providers edit <id> [--name NAME] [--home PATH] [--key-stdin]
  providers enable <id>
  providers disable <id>
  providers models <id>
  providers select <id> <model-id> [--efforts LIST]
  providers deselect <id> <model-id>
  providers remove <id>
  clients list
  clients create <name>
  clients revoke <id>
  network show
  network set [--host HOST] [--port PORT] [--require-key|--no-require-key] [--origin ORIGIN ...|--clear-origins]
  settings apply                      Read a settings command as JSON on stdin
  --help
  --version

Commands write JSON to stdout. Use --key-stdin with a pipe to supply an OpenRouter key.
On headless Linux, enable systemd user lingering to run before the account signs in.
`;

interface NetworkOptions {
  host?: string;
  port?: string;
  origin?: string[];
  'require-key'?: boolean;
  'no-require-key'?: boolean;
  'clear-origins'?: boolean;
}

interface ProviderOptions {
  name?: string;
  home?: string;
  wsl?: string;
  'key-stdin'?: boolean;
}

export async function executeCli(client: ControlClient, args: readonly string[]): Promise<unknown> {
  const [group, action, ...values]: readonly string[] = args;
  const invoke: (command: SettingsCommand) => Promise<Extract<SettingsReply, { ok: true }>['value']> = (command: SettingsCommand): Promise<Extract<SettingsReply, { ok: true }>['value']> => client.invoke(command).then(unwrap);
  const status: () => Promise<RouterStatus> = async (): Promise<RouterStatus> => await invoke({ type: 'status' }) as RouterStatus;

  if (group === 'status' && !action) return status();
  if (group === 'stop' && !action) { await client.stop(); return null; }
  if (group === 'startup' && values.length === 0) {
    if (!action) return { mode: (await status()).startup_mode };
    await invoke({ type: 'save_startup', mode: startupModeSchema.parse(action) });
    return { mode: action };
  }

  if (group === 'settings' && action === 'apply' && values.length === 0) {
    const raw: unknown = parseNativeJson(await readStdin());
    return invoke(commandSchema.parse(raw));
  }

  if (group === 'clients') {
    if (action === 'list' && !values.length) return (await status()).clients;
    if (action === 'create' && values.length === 1) return invoke({ type: 'create_client', name: values[0]! });
    if (action === 'revoke' && values.length === 1) return invoke({ type: 'revoke_client', id: values[0]! });
  }

  if (group === 'network') {
    if (action === 'show' && !values.length) return (await status()).network;
    if (action === 'set') {
      const parsed: NetworkOptions = parseArgs({ args: values, options: {
        host: { type: 'string' }, port: { type: 'string' }, 'require-key': { type: 'boolean' },
        'no-require-key': { type: 'boolean' }, 'clear-origins': { type: 'boolean' }, origin: { type: 'string', multiple: true },
      } }).values;
      if ((parsed['require-key'] && parsed['no-require-key']) || (parsed.origin && parsed['clear-origins'])) throw usage();
      const current: RouterStatus['network'] = (await status()).network;
      const network: RouterStatus['network'] = networkSchema.parse({
        host: parsed.host ?? current.host,
        port: parsed.port === undefined ? current.port : Number(parsed.port),
        require_api_key: parsed['require-key'] ? true : parsed['no-require-key'] ? false : current.require_api_key,
        browser_origins: parsed['clear-origins'] ? [] : parsed.origin ?? current.browser_origins,
      });
      await invoke({ type: 'save_network', network });
      return network;
    }
  }

  if (group === 'providers') {
    if (action === 'list' && !values.length) return (await status()).connections;
    if (action === 'discover' && !values.length) return (await invoke({ type: 'snapshot' }) as SettingsSnapshot).detected_providers;
    if (action === 'use' && values.length === 1) {
      const candidates: DetectedProvider[] = (await invoke({ type: 'snapshot' }) as SettingsSnapshot).detected_providers;
      const detected: DetectedProvider | undefined = candidates.find((candidate: DetectedProvider): boolean =>
        (candidate.connection.wsl_distribution ?? 'native') === values[0] && candidate.status === 'ready');
      if (!detected) throw new RouterError('PROVIDER_UNAVAILABLE', 'The selected Codex installation is unavailable. Run providers discover to check its status.');
      const configured: Connection | undefined = (await status()).connections.find((connection: Connection): boolean =>
        connection.method === 'codex_http' && connection.wsl_distribution === detected.connection.wsl_distribution && !connection.codex_home && !connection.executable);
      if (configured) return configured;
      await invoke({ type: 'save_connection', connection: detected.connection });
      return detected.connection;
    }

    if (action === 'add' || action === 'edit') {
      const parsed: { positionals: string[]; values: ProviderOptions } = parseArgs({ args: values, allowPositionals: true, options: {
        name: { type: 'string' }, home: { type: 'string' }, wsl: { type: 'string' }, 'key-stdin': { type: 'boolean' },
      } });
      if (parsed.positionals.length !== 1 || (!parsed.positionals[0])) throw usage();
      const provider: string = parsed.positionals[0];
      let connection: Connection;
      if (action === 'add') {
        if (provider !== 'codex' && provider !== 'openrouter') throw usage();
        connection = newConnection(provider, parsed.values.wsl);
      } else {
        if (parsed.values.wsl) throw usage();
        connection = findConnection(await status(), provider);
      }

      if (connection.method === 'openrouter_http' ? parsed.values.home !== undefined : parsed.values['key-stdin']) throw usage();
      if (action === 'add' && connection.method === 'openrouter_http' && !parsed.values['key-stdin']) {
        throw new RouterError('API_KEY_REQUIRED', 'Supply the OpenRouter API key on stdin using --key-stdin.');
      }

      const apiKey: string | undefined = parsed.values['key-stdin'] ? (await readStdin()).trim() : undefined;
      if (apiKey === '') throw new RouterError('API_KEY_REQUIRED', 'An OpenRouter API key is required.');
      if (parsed.values.name !== undefined) connection.name = parsed.values.name;
      if (parsed.values.home !== undefined) connection.codex_home = parsed.values.home || undefined;
      await invoke({ type: 'save_connection', connection, api_key: apiKey });
      return connection;
    }

    if (action === 'enable' || action === 'disable' || action === 'remove' || action === 'models') {
      if (values.length !== 1) throw usage();
      const id: string = values[0]!;
      if (action === 'remove') return invoke({ type: 'remove_connection', id });
      if (action === 'models') return invoke({ type: 'refresh_models', id });
      const connection: Connection = findConnection(await status(), id);
      const updated: Connection = { ...connection, exposed: action === 'enable' };
      await invoke({ type: 'save_connection', connection: updated });
      return updated;
    }

    if (action === 'select' || action === 'deselect') {
      const parsed: { positionals: string[]; values: { efforts?: string } } = parseArgs({ args: values, allowPositionals: true, options: { efforts: { type: 'string' } } });
      const [id, modelId]: string[] = parsed.positionals;
      if (parsed.positionals.length !== 2 || !id || !modelId || (action === 'deselect' && parsed.values.efforts !== undefined)) throw usage();
      const connection: Connection = findConnection(await status(), id);
      const models: Model[] = connection.models.filter((model: Model): boolean => model.id !== modelId);
      if (action === 'select') {
        const available: Model[] = await invoke({ type: 'refresh_models', id }) as Model[];
        const model: Model | undefined = available.find((item: Model): boolean => item.id === modelId);
        if (!model) throw new RouterError('MODEL_UNAVAILABLE', 'Run providers models to list available models.');
        models.push(selectModel(model, parsed.values.efforts));
      }

      await invoke({ type: 'save_connection', connection: { ...connection, models } });
      return models;
    }
  }

  throw usage();
}

export function unwrap(reply: SettingsReply): Extract<SettingsReply, { ok: true }>['value'] {
  if (!reply.ok) throw new RouterError('SETTINGS_FAILED', reply.error);
  return reply.value;
}

export function commandError(error: unknown): string {
  if (error instanceof RouterError) return error.message;
  if (error instanceof z.ZodError) {
    const field: string = error.issues[0]?.path.join('.') || 'command';
    return `Invalid ${field}. Check provider-router --help.`;
  }

  return 'Provider Router could not complete the command. Check provider-router --help and the local configuration.';
}

function usage(): RouterError {
  return new RouterError('INVALID_COMMAND', 'Invalid command. Run provider-router --help for usage.');
}

function findConnection(status: RouterStatus, id: string): Connection {
  const connection: Connection | undefined = status.connections.find((item: Connection): boolean => item.id === id);
  if (!connection) throw new RouterError('CONNECTION_UNAVAILABLE', 'The provider is unavailable. Run providers list to check its ID.');
  return connection;
}

function newConnection(provider: 'codex' | 'openrouter', distribution?: string): Connection {
  if (distribution && (provider !== 'codex' || process.platform !== 'win32')) throw usage();
  return {
    id: uuid7(), name: provider === 'codex' ? 'Codex' : 'OpenRouter', method: provider === 'codex' ? 'codex_http' : 'openrouter_http',
    exposed: false, models: [], environment: distribution ? 'wsl' : 'native', ...(distribution ? { wsl_distribution: distribution } : {}),
  };
}

function selectModel(model: Model, efforts?: string): Model {
  if (efforts === undefined) return model;
  const allowed: string[] = [...new Set(efforts.split(',').map((effort: string): string => effort.trim()).filter(Boolean))];
  if (!allowed.length || allowed.some((effort: string): boolean => !model.reasoning_efforts.includes(effort))) {
    throw new RouterError('MODEL_EFFORT_INVALID', 'Choose at least one thinking effort listed for this model.');
  }

  return { ...model, reasoning_efforts: allowed, default_reasoning_effort: model.default_reasoning_effort && allowed.includes(model.default_reasoning_effort) ? model.default_reasoning_effort : allowed[0]! };
}

async function readStdin(): Promise<string> {
  if (process.stdin.isTTY) throw new RouterError('STDIN_REQUIRED', 'Pipe the value to stdin or enter it in the terminal app.');
  const chunks: Buffer[] = [];
  let size: number = 0;
  for await (const value of process.stdin) {
    const buffer: Buffer = Buffer.from(value);
    size += buffer.length;
    if (size > 2 * 1024 * 1024) throw new RouterError('STDIN_TOO_LARGE', 'Input exceeds the settings command size limit.');
    chunks.push(buffer);
  }

  return Buffer.concat(chunks).toString('utf8');
}
