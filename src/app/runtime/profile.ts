/** Locate the account's shared settings and serialize access across application processes. */
import { createHash } from 'node:crypto';
import type { Stats } from 'node:fs';
import { chmod, mkdir, open, realpath, stat, type FileHandle } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { tryLock, unlock } from 'fs-native-extensions';
import { RouterError } from '../common/schemas/contracts.ts';

export interface RouterProfile {
  directory: string;
  id: string;
  socket: string;
  tokenFile: string;
}

export function defaultDataDirectory(): string {
  const home: string = os.homedir();
  if (process.platform === 'win32') return path.join(process.env.APPDATA || path.join(home, 'AppData', 'Roaming'), 'provider-router');
  if (process.platform === 'darwin') return path.join(home, 'Library', 'Application Support', 'provider-router');
  return path.join(process.env.XDG_CONFIG_HOME || path.join(home, '.config'), 'provider-router');
}

export async function openProfile(directory: string): Promise<RouterProfile> {
  if (!path.isAbsolute(directory)) throw new RouterError('SETTINGS_INVALID', 'Use an absolute path for the Provider Router data directory.');
  await mkdir(directory, { recursive: true, mode: 0o700 });
  const canonical: string = await realpath(directory);
  if (process.platform !== 'win32') {
    const metadata: Stats = await stat(canonical);
    if (metadata.uid !== process.getuid?.()) throw new RouterError('SETTINGS_INVALID', 'The data directory must belong to the account running Provider Router.');
    if (metadata.mode & 0o077) {
      if (canonical === path.resolve(directory) && canonical === defaultDataDirectory()) await chmod(canonical, 0o700);
      else throw new RouterError('SETTINGS_INVALID', 'Choose a private data directory with permissions 0700.');
    }
  }

  const id: string = createHash('sha256').update(process.platform === 'win32' ? canonical.toLowerCase() : canonical).digest('hex').slice(0, 32);
  const socket: string = process.platform === 'win32' ? `\\\\.\\pipe\\provider-router-${id}` : path.join(canonical, 'control.sock');
  return { directory: canonical, id, socket, tokenFile: path.join(canonical, 'control.token') };
}

/** Only one process may open the writable store or publish the discovery descriptor. */
export async function lockProfile(profile: RouterProfile): Promise<(() => Promise<void>) | undefined> {
  const file: FileHandle = await open(path.join(profile.directory, 'router.lock'), 'a+', 0o600);
  try {
    if (tryLock(file.fd)) {
      return async (): Promise<void> => {
        try { unlock(file.fd); } finally { await file.close(); }
      };
    }
  } catch (error: unknown) {
    await file.close();
    throw error;
  }

  await file.close();
  return undefined;
}
