#!/usr/bin/env node
/** Headless and interactive entry point for the same Provider Router configuration. */
import { stringifyWireJson } from './app/common/schemas/json-wire.ts';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { parseArgs } from 'node:util';
import { commandError, commandHelp, executeCli, unwrap } from './app/cli-commands.ts';
import { RouterError } from './app/common/schemas/contracts.ts';
import { TerminalScreen } from './app/products/router/presentation/terminal/screen.ts';
import { showTerminalSettings } from './app/products/router/presentation/terminal/settings.ts';
import type { RouterStatus } from './app/products/router/schemas/settings-contract.ts';
import { ControlClient } from './app/runtime/control-client.ts';
import { serveControl } from './app/runtime/control-server.ts';
import type { LaunchCommand } from './app/runtime/launch.ts';
import { defaultDataDirectory, lockProfile, openProfile, type RouterProfile } from './app/runtime/profile.ts';
import { RouterRuntime } from './app/runtime/router-runtime.ts';

declare const PROVIDER_ROUTER_VERSION: string;

async function main(): Promise<void> {
  const args: string[] = process.argv.slice(2);
  const prefix: string[] = args[0] === '--data-dir' ? args.splice(0, 2) : [];
  const parsed: { values: { 'data-dir'?: string } } = parseArgs({ args: prefix, options: { 'data-dir': { type: 'string' } } });
  if (args.length === 1 && (args[0] === '--version' || args[0] === '-v')) { console.log(PROVIDER_ROUTER_VERSION); return; }
  if (args[0] === 'help' || args.includes('--help') || args.includes('-h')) { console.log(commandHelp); return; }
  if (!args.length && (!process.stdin.isTTY || !process.stdout.isTTY)) {
    throw new RouterError('TERMINAL_REQUIRED', 'Open provider-router in an interactive terminal, or run provider-router --help for scriptable commands.');
  }

  const profile: RouterProfile = await openProfile(parsed.values['data-dir'] ?? process.env.PROVIDER_ROUTER_DATA_DIR ?? defaultDataDirectory());
  const launch: LaunchCommand = { executable: process.execPath, script: path.resolve(process.argv[1]!), electron: process.env.ELECTRON_RUN_AS_NODE === '1' };
  const client: ControlClient = new ControlClient(profile, launch, PROVIDER_ROUTER_VERSION);
  const abort: AbortController = new AbortController();
  const cancel: () => void = (): void => abort.abort();
  process.once('SIGINT', cancel);
  process.once('SIGTERM', cancel);

  try {
    if ((args[0] === 'serve' && args.length === 1) || (args[0] === '__serve' && args.length === 2)) {
      const mode: string | undefined = args[0] === 'serve' ? 'foreground' : args[1];
      if (mode !== 'foreground' && mode !== 'background' && mode !== 'boot') {
        throw new RouterError('INVALID_COMMAND', 'Invalid serve mode.');
      }

      let release: (() => Promise<void>) | undefined = await lockProfile(profile);
      // A short offline settings command can own the profile while the new service starts.
      for (let attempt: number = 0; !release && mode !== 'foreground' && attempt < 20 && !abort.signal.aborted; attempt++) {
        await delay(50);
        release = await lockProfile(profile);
      }
      if (!release) {
        if (mode !== 'foreground') return;
        throw new RouterError('ROUTER_RUNNING', 'Provider Router already owns these settings. Run provider-router status or stop.');
      }

      try {
        const runtime: RouterRuntime = await RouterRuntime.open(profile, launch, PROVIDER_ROUTER_VERSION, true);
        await serveControl(profile, runtime, mode, abort.signal);
      } finally { await release(); }
      return;
    }

    if (args.length) {
      console.log(stringifyWireJson(await executeCli(client, args), 2));
      return;
    }

    const screen: TerminalScreen = new TerminalScreen(process.stdin, process.stdout, abort.signal, cancel);
    let view: AbortController | undefined;
    const start: () => Promise<void> = async (): Promise<void> => {
      screen.progress('Starting the router…');
      view?.abort();
      view = new AbortController();
      const status: RouterStatus = unwrap(await client.start()) as RouterStatus;
      await client.attach(AbortSignal.any([abort.signal, view.signal]));
      if (status.error) throw new RouterError('LISTENER_UNAVAILABLE', status.error);
    };

    try {
      try { await start(); }
      catch (error: unknown) { if (!abort.signal.aborted) await screen.message('Startup', [commandError(error)]); }
      await showTerminalSettings(screen, client, { start, stop: (): Promise<void> => client.stop() });
    } finally {
      view?.abort();
      screen.close();
    }
  } catch (error: unknown) {
    if (!abort.signal.aborted) throw error;
  } finally {
    process.off('SIGINT', cancel);
    process.off('SIGTERM', cancel);
  }
}

void main().catch((error: unknown): void => {
  console.error(commandError(error));
  process.exitCode = 1;
});
