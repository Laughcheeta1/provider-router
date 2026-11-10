/** Locate an installed Windows Codex entrypoint without executing a command through cmd.exe. */
import { access, constants, readdir } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { RouterError } from '../../../common/schemas/contracts.ts';

export interface NativeCodexCommand {
  readonly executable: string;
  readonly prefix: string[];
  readonly environment: Record<string, string>;
}

/** Native executables run directly; standard package-manager shims use the installed Node entrypoint. */
export async function resolveNativeCodex(
  executable: string = 'codex',
  options: { readonly platform?: NodeJS.Platform; readonly searchPath?: string; readonly nodeExecutable?: string; readonly homeDirectory?: string } = {},
): Promise<NativeCodexCommand> {
  const platform: NodeJS.Platform = options.platform ?? process.platform;
  if (platform === 'darwin') return resolveMacCodex(executable, options);
  if (platform !== 'win32') return { executable, prefix: [], environment: {} };
  const directories: string[] = path.win32.isAbsolute(executable)
    ? [path.win32.dirname(executable)] : (options.searchPath ?? process.env.PATH ?? '').split(';').filter(Boolean);
  const name: string = path.win32.basename(executable);
  for (const directory of directories) {
    const base: string = path.win32.join(directory.replace(/^"|"$/g, ''), name);
    const candidates: string[] = /\.(?:exe|cmd|bat)$/i.test(base) ? [base] : [`${base}.exe`, `${base}.cmd`];
    for (const candidate of candidates) {
      if (!await exists(candidate)) continue;
      if (/\.exe$/i.test(candidate)) return { executable: candidate, prefix: [], environment: {} };
      const entrypoint: string = path.win32.join(path.win32.dirname(candidate), 'node_modules/@openai/codex/bin/codex.js');
      if (await exists(entrypoint)) {
        return { executable: options.nodeExecutable ?? process.execPath, prefix: [entrypoint], environment: { ELECTRON_RUN_AS_NODE: '1' } };
      }
      throw new RouterError('CODEX_EXECUTABLE_REQUIRED', 'Select the installed codex.exe or its standard Codex package entrypoint.', 503);
    }
  }
  throw new RouterError('CODEX_NOT_INSTALLED', 'Codex is not available on PATH. Select its installed executable.', 503);
}

/** Finder launches do not inherit an interactive shell's Homebrew or Node version-manager PATH. */
async function resolveMacCodex(executable: string, options: { readonly searchPath?: string; readonly homeDirectory?: string }): Promise<NativeCodexCommand> {
  const home: string = options.homeDirectory ?? os.homedir();
  const searchPath: string = options.searchPath ?? process.env.PATH ?? '/usr/bin:/bin:/usr/sbin:/sbin';
  const versionRoot: string = path.join(home, '.nvm/versions/node');
  let versions: string[] = [];
  try { versions = (await readdir(versionRoot)).filter((version: string): boolean => /^v\d+\.\d+\.\d+$/.test(version)).sort((left: string, right: string): number => right.localeCompare(left, 'en', { numeric: true })); }
  catch { /* Version-manager installation is optional. */ }
  const directories: string[] = path.posix.isAbsolute(executable) ? [path.dirname(executable)] : [
    ...searchPath.split(':').filter(Boolean),
    path.join(home, '.local/bin'), path.join(home, '.bun/bin'), path.join(home, '.npm-global/bin'), path.join(home, '.volta/bin'),
    '/opt/homebrew/bin', '/usr/local/bin', ...versions.map((version: string): string => path.join(versionRoot, version, 'bin')),
  ];
  for (const directory of directories) {
    const candidate: string = path.join(directory, path.basename(executable));
    try { await access(candidate, constants.X_OK); }
    catch { continue; }
    return { executable: candidate, prefix: [], environment: { PATH: [directory, '/opt/homebrew/bin', '/usr/local/bin', searchPath].join(':') } };
  }
  throw new RouterError('CODEX_NOT_INSTALLED', 'Codex is not available. Select its installed executable in Advanced settings.', 503);
}

async function exists(filename: string): Promise<boolean> {
  try { await access(filename, constants.F_OK); return true; }
  catch { return false; }
}
