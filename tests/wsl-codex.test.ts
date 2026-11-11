/** Verify Windows/WSL connection paths and cancellation without requiring or mutating a Windows host. */
import { execFileSync } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { connectionSchema } from '../src/app/common/schemas/contracts.ts';
import { decodeWslDistributions, listWslDistributions, prepareWslCodex, resolveWslCodex, resolveWslCredentialHome, translateWslArguments, type WslCodexContext, type WslRunner } from '../src/app/services/provider-service/providers/wsl-codex.ts';
// RevExt: 1
describe('Windows WSL Codex connections', (): void => {
  it('decodes distribution names from wsl.exe UTF-16 output and never launches WSL on other operating systems', async (): Promise<void> => {
    expect(decodeWslDistributions(Buffer.from('\uFEFFUbuntu-24.04\r\nDevelopment Linux\r\nUbuntu-24.04\r\n', 'utf16le'))).toEqual(['Ubuntu-24.04', 'Development Linux']);
    expect(await listWslDistributions('darwin', async (): Promise<never> => { throw new Error('Must not launch WSL'); })).toEqual([]);
  });  // RevExt: 8
// RevExt: 2
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
  });  // RevExt: 9
// RevExt: 3
  it('translates input/schema/output paths without modifying prompt/model values', (): void => {
    const directory: string = 'C:\\Users\\Person\\Temp\\router 1';
    const args: string[] = ['exec', '--model', 'gpt-6-luna', '-C', directory, '-o', `${directory}\\result.json`, '-c', `model_instructions_file=${JSON.stringify(`${directory}\\system.md`)}`];
    expect(translateWslArguments(args, directory, '/mnt/c/Users/Person/Temp/router 1')).toEqual([
      'exec', '--model', 'gpt-6-luna', '-C', '/mnt/c/Users/Person/Temp/router 1', '-o', '/mnt/c/Users/Person/Temp/router 1/result.json', '-c', 'model_instructions_file="/mnt/c/Users/Person/Temp/router 1/system.md"',
    ]);
  });  // RevExt: 10
// RevExt: 4
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
  });  // RevExt: 11
// RevExt: 5
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
  });  // RevExt: 12
// RevExt: 6
  it('resolves direct credential profiles inside the selected WSL distribution', async (): Promise<void> => {
    const run: WslRunner = async (): Promise<{ code: number; stdout: Buffer }> => ({ code: 0, stdout: Buffer.from('/home/user/account profile\n') });
    expect(await resolveWslCredentialHome('Development Linux', '/home/user/account profile', { platform: 'win32', run })).toBe(path.win32.join('\\\\wsl.localhost', 'Development Linux', 'home', 'user', 'account profile'));
    await expect(resolveWslCredentialHome('Ubuntu', undefined, { platform: 'linux', run })).rejects.toMatchObject({ code: 'WSL_WINDOWS_ONLY' });
    await expect(resolveWslCredentialHome('../Ubuntu', undefined, { platform: 'win32', run })).rejects.toMatchObject({ code: 'WSL_DISTRIBUTION_INVALID' });
    await expect(resolveWslCredentialHome('Ubuntu', undefined, { platform: 'win32', run: async () => ({ code: 1, stdout: Buffer.from('') }) })).rejects.toMatchObject({ code: 'WSL_PATH_INVALID' });
  });  // RevExt: 13
// RevExt: 7
  it('accepts WSL direct connections with a selected distribution and rejects non-Codex WSL targets', (): void => {
    expect(connectionSchema.safeParse({ id: '01960000-0000-7000-8000-000000000001', name: 'WSL', method: 'codex_http', exposed: false, models: [], environment: 'wsl', wsl_distribution: 'Ubuntu' }).success).toBe(true);
    expect(connectionSchema.safeParse({ id: '01960000-0000-7000-8000-000000000001', name: 'WSL', method: 'openrouter_http', exposed: false, models: [], environment: 'wsl', wsl_distribution: 'Ubuntu' }).success).toBe(false);
  });  // RevExt: 14
});
