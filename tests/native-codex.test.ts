/** Verify GUI-started macOS discovery without evaluating the user's shell configuration. */
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { expect, it } from 'vitest';
import { resolveNativeCodex, type NativeCodexCommand } from '../src/app/services/provider-service/providers/native-codex.ts';

it.skipIf(process.platform === 'win32')('finds a user-installed CLI when a Finder-style PATH omits it', async (): Promise<void> => {
  const home: string = await mkdtemp(path.join(os.tmpdir(), 'router-native-cli-'));
  try {
    const executable: string = path.join(home, '.local/bin/codex');
    await mkdir(path.dirname(executable), { recursive: true });
    await writeFile(executable, '#!/bin/sh\nexit 0\n', { mode: 0o700 });
    const command: NativeCodexCommand = await resolveNativeCodex('codex', { platform: 'darwin', homeDirectory: home, searchPath: path.join(home, 'empty') });
    expect(command.executable).toBe(executable);
    expect(command.environment.PATH.split(':')[0]).toBe(path.dirname(executable));
    expect(command.prefix).toEqual([]);
  } finally { await rm(home, { recursive: true, force: true }); }
});

it.skipIf(process.platform === 'win32')('honors an explicit executable with spaces as a single native path', async (): Promise<void> => {
  const home: string = await mkdtemp(path.join(os.tmpdir(), 'router-native-cli-'));
  try {
    const executable: string = path.join(home, 'My Tools/codex');
    await mkdir(path.dirname(executable), { recursive: true });
    await writeFile(executable, '#!/bin/sh\nexit 0\n', { mode: 0o700 });
    const command: NativeCodexCommand = await resolveNativeCodex(executable, { platform: 'darwin', homeDirectory: home, searchPath: '' });
    expect(command.executable).toBe(executable);
    expect(command.prefix).toEqual([]);
  } finally { await rm(home, { recursive: true, force: true }); }
});
