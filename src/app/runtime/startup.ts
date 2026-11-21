/** Register the selected profile with the host's startup mechanism. */
import { execFile } from 'node:child_process';
import { mkdir, rename, rm, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { RouterError } from '../common/schemas/contracts.ts';
import { launchArgs, type LaunchCommand } from './launch.ts';
import type { RouterProfile } from './profile.ts';

const execute: typeof execFile.__promisify__ = promisify(execFile);

export async function configureStartup(profile: RouterProfile, command: LaunchCommand, enable: boolean): Promise<void> {
  const args: string[] = [command.executable, ...launchArgs(command, profile.directory, 'boot')];
  try {
    if (process.platform === 'linux') await configureSystemd(profile.id, args, command.electron, enable);
    else if (process.platform === 'darwin') await configureLaunchAgent(profile.id, args, command.electron, enable);
    else if (process.platform === 'win32') await configureWindows(profile.id, args, command.electron, enable);
    else if (enable) throw new Error('Unsupported startup platform');
  } catch {
    throw new RouterError('STARTUP_UNAVAILABLE', 'Provider Router could not update this account’s startup registration.');
  }
}

/** Enabling this user unit also works on headless systems with user lingering enabled. */
async function configureSystemd(id: string, args: string[], electron: boolean, enable: boolean): Promise<void> {
  const directory: string = path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config'), 'systemd', 'user');
  const name: string = `provider-router-${id}.service`;
  const service: string = path.join(directory, name);
  const wants: string = path.join(directory, 'default.target.wants');
  const link: string = path.join(wants, name);
  if (enable) {
    await mkdir(wants, { recursive: true, mode: 0o700 });
    const command: string = args.map(systemdQuote).join(' ');
    const environment: string[] = ['PATH', 'CODEX_HOME'].flatMap((key: string): string[] =>
      process.env[key] ? [`Environment=${systemdQuote(`${key}=${process.env[key]}`)}`] : []);
    const content: string = [
      '[Unit]', 'Description=Provider Router', '[Service]', 'Type=exec',
      ...(electron ? ['Environment=ELECTRON_RUN_AS_NODE=1'] : []),
      ...environment,
      `ExecStart=:${command}`, '[Install]', 'WantedBy=default.target', '',
    ].join('\n');
    await writeFile(`${service}.tmp`, content, { mode: 0o600 });
    await rename(`${service}.tmp`, service);
    try { await symlink(`../${name}`, link); }
    catch (error: unknown) { if (!(error instanceof Error) || !('code' in error) || error.code !== 'EEXIST') throw error; }
  } else {
    await rm(link, { force: true });
    await rm(service, { force: true });
  }
  // A user manager may be absent in WSL or during offline configuration.
  await execute('systemctl', ['--user', 'daemon-reload'], { timeout: 3000 }).catch((): void => {});
}

function systemdQuote(value: string): string {
  return JSON.stringify(value.replaceAll('%', '%%'));
}

async function configureLaunchAgent(id: string, args: string[], electron: boolean, enable: boolean): Promise<void> {
  const directory: string = path.join(os.homedir(), 'Library', 'LaunchAgents');
  const label: string = `org.providerrouter.${id}`;
  const target: string = path.join(directory, `${label}.plist`);
  if (!enable) { await rm(target, { force: true }); return; }
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const xmlArgs: string = args.map((arg: string): string => `<string>${xml(arg)}</string>`).join('');
  const exported: string = ['PATH', 'CODEX_HOME'].map((key: string): string =>
    process.env[key] ? `<key>${key}</key><string>${xml(process.env[key]!)}</string>` : '').join('');
  const environment: string = `<key>EnvironmentVariables</key><dict>${electron ? '<key>ELECTRON_RUN_AS_NODE</key><string>1</string>' : ''}${exported}</dict>`;
  await writeFile(`${target}.tmp`, `<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>Label</key><string>${label}</string><key>ProgramArguments</key><array>${xmlArgs}</array><key>RunAtLoad</key><true/>${environment}</dict></plist>\n`, { mode: 0o600 });
  await rename(`${target}.tmp`, target);
}

function xml(value: string): string {
  return value.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
}

async function configureWindows(id: string, args: string[], electron: boolean, enable: boolean): Promise<void> {
  const run: string = 'HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run';
  const name: string = `ProviderRouter-${id}`;
  if (!enable) {
    const existing: boolean = await execute('reg.exe', ['query', run, '/v', name]).then((): boolean => true, (): boolean => false);
    if (existing) await execute('reg.exe', ['delete', run, '/v', name, '/f']);
    return;
  }

  // Windows launches Electron desktop mode unless its child environment sets this flag.
  const escaped: string = args.map((arg: string): string => `"${arg}"`).join(' ');
  if (electron) {
    const [executable, , , directory]: string[] = args;
    await execute('reg.exe', ['add', run, '/v', name, '/t', 'REG_SZ', '/d', `"${executable}" --background --router-data-dir "${directory}"`, '/f']);
  } else {
    await execute('reg.exe', ['add', run, '/v', name, '/t', 'REG_SZ', '/d', escaped, '/f']);
  }
}
