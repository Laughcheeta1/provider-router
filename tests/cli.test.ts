/** Exercise the built terminal entry point and live API with isolated Linux profiles. */
import { execFile, spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { createServer, type AddressInfo, type Server } from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { promisify } from 'node:util';
import OpenAI from 'openai';
import { beforeAll, expect, it } from 'vitest';
import type { RouterStatus } from '../src/app/products/router/schemas/settings-contract.ts';
import { createCredentialVault } from '../src/app/runtime/credential-vault.ts';

const execute: typeof execFile.__promisify__ = promisify(execFile);
const binary: string = process.execPath;
const cli: string = path.resolve('dist/cli.cjs');
const linux: ReturnType<typeof it.runIf> = it.runIf(process.platform === 'linux');

beforeAll(async (): Promise<void> => {
  if (process.platform === 'linux') await execute(binary, ['scripts/build.ts']);
}, 15_000);

linux('shares settings with the foreground API and revokes live OpenAI SDK access', async (): Promise<void> => {
  await withProfile(async (profile: string): Promise<void> => {
    const router: ReturnType<typeof commands> = commands(profile);
    const port: number = await localPort();
    await router('network', 'set', '--port', String(port), '--require-key');
    const offline: RouterStatus = await router('status');
    expect(offline.endpoint).toBeNull();

    const server: ChildProcessWithoutNullStreams = spawn(binary, [cli, '--data-dir', profile, 'serve']);
    try {
      const online: RouterStatus = await running(router, true);
      expect(online.instance_id).toBe(offline.instance_id);
      const client: { id: string; token: string } = await router('clients', 'create', 'SDK client');
      const sdk: OpenAI = new OpenAI({ apiKey: client.token, baseURL: `${online.endpoint}/v1`, maxRetries: 0 });
      expect((await sdk.models.list()).data).toEqual([]);
      expect(await router('clients', 'list')).toMatchObject([{ id: client.id, name: 'SDK client' }]);
      await router('clients', 'revoke', client.id);
      await expect(sdk.models.list()).rejects.toMatchObject({ status: 401 });
      await router('stop');
      expect((await running(router, false)).endpoint).toBeNull();
    } finally {
      server.kill();
    }
  });
}, 20_000);

linux('keeps the API until the last terminal app exits and honors the persistent mode', async (): Promise<void> => {
  await withProfile(async (profile: string): Promise<void> => {
    const router: ReturnType<typeof commands> = commands(profile);
    await router('network', 'set', '--port', String(await localPort()));
    const first: ChildProcessWithoutNullStreams = await terminal(profile);
    try {
      const startupMenu: Promise<void> = waitForTerminal(first, ['Run on startup', 'Run only while open']);
      first.stdin.write('s');
      await startupMenu;
      const settings: Promise<void> = waitForTerminal(first, ['Client keys', 'API access']);
      first.stdin.write('\x1b[B\r');
      await settings;
      expect(await router('startup')).toEqual({ mode: 'while-open' });

      const second: ChildProcessWithoutNullStreams = await terminal(profile);
      try {
        await running(router, true);
        await exitTerminal(first);
        expect((await router<RouterStatus>('status')).endpoint).toBeTruthy();
        await exitTerminal(second);
        await running(router, false);
      } finally {
        second.kill();
      }
    } finally {
      first.kill();
    }

    await router('startup', 'keep-after-open');
    const persistent: ChildProcessWithoutNullStreams = await terminal(profile);
    try {
      await exitTerminal(persistent);
      expect((await router<RouterStatus>('status')).endpoint).toBeTruthy();
    } finally {
      persistent.kill();
    }
  });
}, 20_000);

linux('registers and removes the selected account startup service', async (): Promise<void> => {
  await withProfile(async (profile: string): Promise<void> => {
    const config: string = path.join(profile, 'config');
    const router: ReturnType<typeof commands> = commands(profile, { XDG_CONFIG_HOME: config, XDG_RUNTIME_DIR: profile });
    await router('startup', 'startup');
    expect(await router('startup')).toEqual({ mode: 'startup' });
    const userUnits: string = path.join(config, 'systemd', 'user');
    const enabled: string = path.join(userUnits, 'default.target.wants');
    const services: string[] = await readdir(enabled);
    expect(services).toHaveLength(1);
    expect(await readFile(path.join(enabled, services[0]!), 'utf8')).toContain('WantedBy=default.target');
    await router('startup', 'while-open');
    expect(await readdir(enabled)).toEqual([]);
    expect(await router('startup')).toEqual({ mode: 'while-open' });
  });
}, 15_000);

linux('saves an OpenRouter key from stdin and unlocks it after reopening the vault', async (): Promise<void> => {
  await withProfile(async (profile: string): Promise<void> => {
    const secret: string = 'synthetic-provider-key-for-local-testing';
    const child: ChildProcessWithoutNullStreams = spawn(binary, [cli, '--data-dir', profile, 'providers', 'add', 'openrouter', '--key-stdin']);
    child.stdin.end(secret + '\n');
    expect(await exitCode(child)).toBe(0);
    const content: string = await readFile(path.join(profile, 'settings.json'), 'utf8');
    expect(content).not.toContain(secret);
    const settings: { secrets: Record<string, string> } = JSON.parse(content);
    const encrypted: string = Object.values(settings.secrets)[0]!;
    expect(createCredentialVault(profile).decrypt(Buffer.from(encrypted, 'base64'))).toBe(secret);
  });
});

function commands(profile: string, environment: NodeJS.ProcessEnv = {}): <T>(...args: string[]) => Promise<T> {
  return async <T>(...args: string[]): Promise<T> => {
    const { stdout }: { stdout: string } = await execute(binary, [cli, '--data-dir', profile, ...args], { env: { ...process.env, ...environment }, timeout: 7000 });
    return JSON.parse(stdout) as T;
  };
}

async function withProfile(task: (profile: string) => Promise<void>): Promise<void> {
  const profile: string = await mkdtemp(path.join(os.tmpdir(), 'pr-cli-test-'));
  try { await task(profile); }
  finally {
    await commands(profile)('stop').catch((): void => {});
    await rm(profile, { recursive: true, force: true });
  }
}

async function localPort(): Promise<number> {
  const server: Server = createServer();
  await new Promise<void>((resolve, reject): void => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const port: number = (server.address() as AddressInfo).port;
  await new Promise<void>((resolve): void => { server.close((): void => resolve()); });
  return port;
}

async function running(router: ReturnType<typeof commands>, expected: boolean): Promise<RouterStatus> {
  for (let attempt: number = 0; attempt < 60; attempt++) {
    const status: RouterStatus = await router('status');
    if (Boolean(status.endpoint) === expected) return status;
    await delay(50);
  }

  throw new Error(`The router did not become ${expected ? 'available' : 'stopped'}.`);
}

function shellQuote(value: string): string {
  return "'" + value.replaceAll("'", "'\\''") + "'";
}

async function terminal(profile: string): Promise<ChildProcessWithoutNullStreams> {
  const child: ChildProcessWithoutNullStreams = spawn('script', ['-qec', ['exec', ...[binary, cli, '--data-dir', profile].map(shellQuote)].join(' '), '/dev/null']);
  try { await waitForTerminal(child, ['Startup behavior', 'Client keys']); }
  catch (error: unknown) { child.kill(); throw error; }
  return child;
}

async function waitForTerminal(child: ChildProcessWithoutNullStreams, labels: readonly string[]): Promise<void> {
  return new Promise<void>((resolve, reject): void => {
    let output: string = '';
    const timer: NodeJS.Timeout = setTimeout((): void => { cleanup(); reject(new Error('Terminal settings did not open.')); }, 6500);
    const data: (chunk: Buffer) => void = (chunk: Buffer): void => {
      output = (output + chunk.toString()).slice(-8192);
      if (labels.every((label: string): boolean => output.includes(label))) { cleanup(); resolve(); }
    };
    const failed: (error: Error) => void = (error: Error): void => { cleanup(); reject(error); };
    const cleanup: () => void = (): void => { clearTimeout(timer); child.stdout.off('data', data); child.off('error', failed); };
    child.stdout.on('data', data);
    child.once('error', failed);
  });
}

async function exitTerminal(child: ChildProcessWithoutNullStreams): Promise<void> {
  const exit: Promise<number | null> = exitCode(child);
  child.stdin.write('\x03');
  expect(await exit).toBe(0);
}

async function exitCode(child: ChildProcessWithoutNullStreams): Promise<number | null> {
  if (child.exitCode !== null) return child.exitCode;
  return new Promise((resolve, reject): void => { child.once('exit', resolve); child.once('error', reject); });
}
