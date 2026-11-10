/** Run a Windows Router connection inside a selected WSL distribution without copying its login. */
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { RouterError } from '../../../common/schemas/contracts.ts';

export interface WslCommandResult { readonly code: number; readonly stdout: Buffer }
export type WslRunner = (args: readonly string[]) => Promise<WslCommandResult>;
export interface WslCodexContext {
  readonly distribution: string;
  readonly codexHome: string;
  readonly catalogDirectory: string;
  readonly executable: string;
  readonly searchPath: string;
}
export interface WslCodexLaunch {
  readonly executable: string;
  readonly args: string[];
  cancel(): Promise<void>;
}

// Only this fixed script runs in a shell. Connection values remain separate argv values.
const inspectScript: string = [
  'set -eu',
  'command -v setsid >/dev/null',
  'cli="$1"',
  'if [ -z "$cli" ]; then cli=$(command -v codex || true); fi',
  // Windows app directories are inherited by WSL; their shell shims launch Windows Node.
  // A WSL connection must select the Linux installation rather than switching environments.
  'case "$cli" in /mnt/[a-zA-Z]/*) cli="";; esac',
  'if [ -z "$cli" ]; then for candidate in "$HOME/.local/bin/codex" "$HOME/.bun/bin/codex" "$HOME/.npm-global/bin/codex" "$HOME"/.nvm/versions/node/*/bin/codex; do if [ -x "$candidate" ]; then cli="$candidate"; fi; done; fi',
  '[ -n "$cli" ] && [ -x "$cli" ]',
  'home="${2:-${CODEX_HOME:-$HOME/.codex}}"',
  'home=$(realpath -m -- "$home")',
  'printf "%s\\0%s\\0%s\\0" "$home" "$cli" "$PATH"',
].join('\n');

/** Decode wsl.exe's UTF-16 distribution list without treating user names as executable text. */
export function decodeWslDistributions(output: Buffer): string[] {
  const text: string = output.includes(0) ? output.toString('utf16le') : output.toString('utf8');
  return [...new Set(text.replace(/^\uFEFF/, '').split(/\r?\n/).map((name: string): string => name.trim())
    .filter((name: string): boolean => Boolean(name) && validDistribution(name)))];
}

/** Discover Windows-native WSL installations; this does not enable a Linux distribution target. */
export async function listWslDistributions(platform: NodeJS.Platform = process.platform, run: WslRunner = runWsl): Promise<string[]> {
  if (platform !== 'win32') return [];
  try {
    const result: WslCommandResult = await run(['--list', '--quiet']);
    return result.code === 0 ? decodeWslDistributions(result.stdout) : [];
  } catch { return []; }
}

/** Resolve the selected distribution's CLI and credential directory in its own environment. */
export async function resolveWslCodex(
  distribution: string,
  options: { readonly executable?: string; readonly codexHome?: string; readonly platform?: NodeJS.Platform; readonly run?: WslRunner } = {},
): Promise<WslCodexContext> {
  if ((options.platform ?? process.platform) !== 'win32') throw new RouterError('WSL_WINDOWS_ONLY', 'WSL connections require Windows.');
  if (!validDistribution(distribution)) throw new RouterError('WSL_DISTRIBUTION_INVALID', 'Choose an installed WSL distribution.');
  const result: WslCommandResult = await (options.run ?? runWsl)([
    '--distribution', distribution, '--exec', '/bin/sh', '-lc', inspectScript, 'provider-router', options.executable ?? '', options.codexHome ?? '',
  ]);
  const fields: string[] = result.stdout.toString('utf8').split('\0');
  if (result.code !== 0 || fields.length < 4 || !fields[0]?.startsWith('/') || !fields[1]?.startsWith('/')
    || !fields[2] || fields.slice(0, 3).some((field: string): boolean => /[\r\n]/.test(field))) {
    throw new RouterError('WSL_CODEX_UNAVAILABLE', 'Codex is unavailable in this WSL distribution. Check its installed CLI and login.', 503);
  }
  return {
    distribution,
    codexHome: fields[0]!,
    catalogDirectory: path.win32.join('\\\\wsl.localhost', distribution, fields[0]!.replaceAll('/', '\\')),
    executable: fields[1]!,
    searchPath: `${path.posix.dirname(fields[1]!)}:${fields[2]!}`,
  };
}

/** Translate only temporary input/output paths, leaving model names and arbitrary prompt text untouched. */
export function translateWslArguments(args: readonly string[], windowsDirectory: string, linuxDirectory: string): string[] {
  if (!linuxDirectory.startsWith('/') || /[\r\n\0]/.test(linuxDirectory)) throw new RouterError('WSL_PATH_INVALID', 'The WSL temporary directory could not be resolved.');
  const replace: (value: string) => string = (value: string): string => value === windowsDirectory || value.startsWith(`${windowsDirectory}\\`) || value.startsWith(`${windowsDirectory}/`)
    ? linuxDirectory + value.slice(windowsDirectory.length).replaceAll('\\', '/') : value;
  return args.map((argument: string): string => {
    const config: RegExpExecArray | null = /^(model_instructions_file|model_catalog_json|log_dir|sqlite_home)=(.*)$/.exec(argument);
    if (!config) return replace(argument);
    const value: unknown = JSON.parse(config[2]!);
    if (typeof value !== 'string') throw new RouterError('WSL_PATH_INVALID', 'The Codex temporary path is invalid.');
    return `${config[1]}=${JSON.stringify(replace(value))}`;
  });
}

/** Keep a Linux process-group receipt so Windows cancellation also terminates Codex inside WSL. */
export async function prepareWslCodex(
  context: WslCodexContext,
  args: readonly string[],
  directory: string,
  run: WslRunner = runWsl,
): Promise<WslCodexLaunch> {
  const mapped: WslCommandResult = await run(['--distribution', context.distribution, '--exec', 'wslpath', '-u', directory]);
  const linuxDirectory: string = mapped.stdout.toString('utf8').trim();
  if (mapped.code !== 0 || !linuxDirectory.startsWith('/')) throw new RouterError('WSL_PATH_INVALID', 'The temporary request cannot be accessed from WSL.');
  const groupFile: string = path.join(directory, 'wsl-process-group');
  const linuxGroupFile: string = `${linuxDirectory}/wsl-process-group`;
  const cancelledFile: string = path.join(directory, 'wsl-cancelled');
  let cancellation: Promise<void> | undefined;
  return {
    executable: 'wsl.exe',
    args: ['--distribution', context.distribution, '--exec', 'setsid', '--fork', '--wait', '/bin/sh', '-c',
      'set -eu; umask 077; printf "%s" "$$" > "$1"; if [ -e "$2" ]; then exit 130; fi; shift 2; exec "$@"', 'provider-router', linuxGroupFile, `${linuxDirectory}/wsl-cancelled`,
      'env', `CODEX_HOME=${context.codexHome}`, `PATH=${context.searchPath}`, context.executable,
      ...translateWslArguments(args, directory, linuxDirectory)],
    cancel: (): Promise<void> => {
      cancellation ??= writeFile(cancelledFile, 'cancelled', { mode: 0o600 }).then((): Promise<void> => cancelWslGroup(context.distribution, groupFile, run));
      return cancellation;
    },
  };
}

/** Terminate only the Linux group created for this request, including a short startup race. */
async function cancelWslGroup(distribution: string, groupFile: string, run: WslRunner): Promise<void> {
  let pid: string = '';
  for (let attempt: number = 0; attempt < 40; attempt += 1) {
    try { pid = (await readFile(groupFile, 'utf8')).trim(); }
    catch { /* The wrapper may still be starting when the HTTP request disconnects. */ }
    if (/^[1-9]\d{0,9}$/.test(pid) && Number(pid) > 1) break;
    await delay(25);
  }
  if (!/^[1-9]\d{0,9}$/.test(pid) || Number(pid) <= 1) return;
  await run(['--distribution', distribution, '--exec', '/bin/kill', '-TERM', '--', `-${pid}`]);
  await delay(150);
  await run(['--distribution', distribution, '--exec', '/bin/kill', '-KILL', '--', `-${pid}`]);
}

/** Bound WSL discovery commands and suppress upstream diagnostics that may contain local information. */
async function runWsl(args: readonly string[]): Promise<WslCommandResult> {
  return new Promise<WslCommandResult>((resolve, reject): void => {
    const child: ChildProcessWithoutNullStreams = spawn('wsl.exe', [...args], { shell: false, windowsHide: true, stdio: 'pipe' });
    const chunks: Buffer[] = [];
    let bytes: number = 0;
    const deadline: ReturnType<typeof setTimeout> = setTimeout((): void => { child.kill(); reject(new RouterError('WSL_TIMEOUT', 'WSL did not respond.', 503)); }, 15000);
    child.once('error', (): void => { clearTimeout(deadline); reject(new RouterError('WSL_UNAVAILABLE', 'WSL is unavailable on this computer.', 503)); });
    child.stdout.on('data', (chunk: Buffer): void => { bytes += chunk.length; if (bytes > 1024 * 1024) child.kill(); else chunks.push(chunk); });
    child.stderr.resume();
    child.stdin.end();
    child.once('close', (code: number | null): void => { clearTimeout(deadline); resolve({ code: bytes > 1024 * 1024 ? 1 : code ?? 1, stdout: Buffer.concat(chunks) }); });
  });
}

function validDistribution(value: string): boolean { return /^(?!\.{1,2}$)[A-Za-z0-9][A-Za-z0-9._ ()-]{0,127}$/.test(value); }
