/** Verify Windows/WSL connection paths and cancellation without requiring or mutating a Windows host. */
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { connectionSchema } from '../src/app/common/schemas/contracts.ts';
import { decodeWslDistributions, listWslDistributions, prepareWslCodex, resolveWslCodex, translateWslArguments, type WslCodexContext, type WslRunner } from '../src/app/services/provider-service/providers/wsl-codex.ts';

describe('Windows WSL Codex connections', (): void => {
  it('decodes distribution names from wsl.exe UTF-16 output and never launches WSL on other operating systems', async (): Promise<void> => {
    expect(decodeWslDistributions(Buffer.from('\uFEFFUbuntu-24.04\r\nDevelopment Linux\r\nUbuntu-24.04\r\n', 'utf16le'))).toEqual(['Ubuntu-24.04', 'Development Linux']);
    expect(await listWslDistributions('darwin', async (): Promise<never> => { throw new Error('Must not launch WSL'); })).toEqual([]);
  });

  it('keeps the selected distribution, Linux executable and login directory in separate process arguments', async (): Promise<void> => {
    const calls: readonly string[][] = [];
    const mutableCalls: string[][] = calls as string[][];
    const run: WslRunner = async (args): Promise<{ code: number; stdout: Buffer }> => {
      mutableCalls.push([...args]);
      return { code: 0, stdout: Buffer.from('/home/user/.codex\0/home/user/.nvm/versions/node/v24/bin/codex\0/usr/bin:/bin\0') };
    };
    const context: WslCodexContext = await resolveWslCodex('Development Linux', { platform: 'win32', run });
    expect(context.catalogDirectory).toBe('\\\\wsl.localhost\\Development Linux\\home\\user\\.codex');
    expect(context.searchPath.startsWith('/home/user/.nvm/versions/node/v24/bin:')).toBe(true);
    expect(calls[0]!.slice(0, 4)).toEqual(['--distribution', 'Development Linux', '--exec', '/bin/sh']);
    await expect(resolveWslCodex('Ubuntu/../../other', { platform: 'win32', run })).rejects.toThrow('installed WSL distribution');
  });

  it('translates input/schema/output paths without modifying prompt/model values', (): void => {
    const directory: string = 'C:\\Users\\Person\\Temp\\router 1';
    const args: string[] = ['exec', '--model', 'gpt-6-luna', '-C', directory, '-o', `${directory}\\result.json`, '-c', `model_instructions_file=${JSON.stringify(`${directory}\\system.md`)}`];
    expect(translateWslArguments(args, directory, '/mnt/c/Users/Person/Temp/router 1')).toEqual([
      'exec', '--model', 'gpt-6-luna', '-C', '/mnt/c/Users/Person/Temp/router 1', '-o', '/mnt/c/Users/Person/Temp/router 1/result.json', '-c', 'model_instructions_file="/mnt/c/Users/Person/Temp/router 1/system.md"',
    ]);
  });

  it.skipIf(process.platform !== 'linux')('selects the Linux CLI when WSL inherited a Windows launcher on PATH', async (): Promise<void> => {
    const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-wsl-home-'));
    try {
      const executable: string = path.join(directory, '.local/bin/codex');
      await mkdir(path.dirname(executable), { recursive: true });
      await writeFile(executable, '#!/bin/sh\nexit 0\n', { mode: 0o700 });
      const run: WslRunner = async (args): Promise<{ code: number; stdout: Buffer }> => ({
        code: 0,
        stdout: execFileSync('/bin/sh', ['-c', args[5]!, ...args.slice(6)], { env: { ...process.env, HOME: directory, CODEX_HOME: path.join(directory, '.codex') } }),
      });
      const context: WslCodexContext = await resolveWslCodex('Ubuntu', { platform: 'win32', executable: '/mnt/c/Program Files/nodejs/codex', run });
      expect(context.executable).toBe(executable);
      expect(context.codexHome).toBe(path.join(directory, '.codex'));
    } finally { await rm(directory, { recursive: true, force: true }); }
  });

  it('cancels the request-owned Linux process group instead of terminating the distribution', async (): Promise<void> => {
    const directory: string = await mkdtemp(path.join(os.tmpdir(), 'router-wsl-test-'));
    try {
      await writeFile(path.join(directory, 'wsl-process-group'), '45678');
      const calls: string[][] = [];
      const run: WslRunner = async (args): Promise<{ code: number; stdout: Buffer }> => {
        calls.push([...args]); return { code: 0, stdout: Buffer.from('/mnt/c/Temp/router\n') };
      };
      const context: WslCodexContext = { distribution: 'Ubuntu', codexHome: '/home/user/.codex', catalogDirectory: '\\\\wsl.localhost\\Ubuntu\\home\\user\\.codex', executable: '/home/user/bin/codex', searchPath: '/home/user/bin:/usr/bin' };
      const launch = await prepareWslCodex(context, ['exec', '-C', directory, '-'], directory, run);
      expect(launch.executable).toBe('wsl.exe');
      expect(launch.args).toContain('CODEX_HOME=/home/user/.codex');
      await launch.cancel(); await launch.cancel();
      expect(calls.slice(1)).toEqual([
        ['--distribution', 'Ubuntu', '--exec', '/bin/kill', '-TERM', '--', '-45678'],
        ['--distribution', 'Ubuntu', '--exec', '/bin/kill', '-KILL', '--', '-45678'],
      ]);
      expect(calls.flat()).not.toContain('--terminate');
    } finally { await rm(directory, { recursive: true, force: true }); }
  });

  it('rejects a WSL HTTP connection rather than reading or refreshing Linux credentials through Windows', (): void => {
    expect(connectionSchema.safeParse({ id: '01960000-0000-7000-8000-000000000001', name: 'WSL', method: 'codex_http', exposed: false, models: [], environment: 'wsl', wsl_distribution: 'Ubuntu' }).success).toBe(false);
  });
});
