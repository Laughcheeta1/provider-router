/** Package the headless application and the host Node runtime as a relocatable archive. */
import { execFileSync } from 'node:child_process';
import { chmod, cp, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

const platform: NodeJS.Platform = process.platform;
const architecture: string = process.arch;
if (!['linux', 'win32', 'darwin'].includes(platform) || !['x64', 'arm64'].includes(architecture)) {
  throw new Error(`Unsupported CLI archive target: ${platform}-${architecture}`);
}

const manifest: { name: string; version: string } = JSON.parse(await readFile('package.json', 'utf8'));
const name: string = `provider-router-${manifest.version}-cli-${platform}-${architecture}`;
const output: string = path.resolve(process.argv[2] ?? 'release');
const temporary: string = await mkdtemp(path.join(os.tmpdir(), 'provider-router-package-'));
const deployed: string = path.join(temporary, 'deployed');
const bundle: string = path.join(temporary, name);
const runtime: string = path.join(bundle, 'runtime');
const windows: boolean = platform === 'win32';
const runtimeName: string = windows ? 'node.exe' : 'node';

try {
  await mkdir(output, { recursive: true });
  await mkdir(runtime, { recursive: true });
  // The pnpm executable can be a .cmd shim on Windows. Arguments are kept fixed apart from the internally generated path.
  execFileSync(windows ? 'cmd.exe' : 'pnpm', [
    ...(windows ? ['/d', '/c', 'pnpm'] : []),
    '--config.node-linker=hoisted', '--filter', manifest.name, 'deploy', '--prod', '--ignore-scripts', deployed,
  ], { stdio: 'inherit' });

  await cp(path.join(deployed, 'node_modules'), path.join(bundle, 'node_modules'), {
    recursive: true,
    filter: (source: string): boolean => includeDependency(source),
  });
  await cp(process.execPath, path.join(runtime, runtimeName));
  const nodeDirectory: string = path.dirname(process.execPath);
  const license: string = await stat(path.join(nodeDirectory, 'LICENSE')).then((): string => path.join(nodeDirectory, 'LICENSE'), (): string => path.join(nodeDirectory, '..', 'LICENSE'));
  await cp(license, path.join(runtime, 'LICENSE'));
  await cp('LICENSE', path.join(bundle, 'LICENSE'));
  await cp('NOTICE.md', path.join(bundle, 'NOTICE.md'));
  await mkdir(path.join(bundle, 'dist'));
  await cp('dist/cli.cjs', path.join(bundle, 'dist', 'cli.cjs'));
  await cp('dist/document-worker.mjs', path.join(bundle, 'dist', 'document-worker.mjs'));
  if (windows) {
    await writeFile(path.join(bundle, 'provider-router.cmd'), '@echo off\r\n"%~dp0runtime\\node.exe" "%~dp0dist\\cli.cjs" %*\r\n');
  } else {
    const launcher: string = path.join(bundle, 'provider-router');
    await writeFile(launcher, getUnixLauncher());
    await chmod(launcher, 0o755);
  }

  const binary: string = path.join(runtime, runtimeName);
  execFileSync(binary, [path.join(bundle, 'dist', 'cli.cjs'), '--version'], { stdio: 'inherit' });
  const archive: string = path.join(output, `${name}.${windows ? 'zip' : 'tar.gz'}`);
  execFileSync('tar', windows
    ? ['--format=zip', '-cf', archive, '-C', temporary, name]
    : ['-czf', archive, '-C', temporary, name], { stdio: 'inherit' });
  console.log(archive);
} finally {
  await rm(temporary, { recursive: true, force: true });
}

function getUnixLauncher(): string {
  return [
    '#!/bin/sh',
    'set -eu',
    'app=$0',
    'while [ -L "$app" ]; do',
    '  directory=$(CDPATH= cd -P -- "$(dirname -- "$app")" && pwd)',
    '  target=$(readlink "$app")',
    '  case "$target" in /*) app=$target ;; *) app=$directory/$target ;; esac',
    'done',
    'directory=$(CDPATH= cd -P -- "$(dirname -- "$app")" && pwd)',
    'exec "$directory/runtime/node" "$directory/dist/cli.cjs" "$@"',
    '',
  ].join('\n');
}

function includeDependency(source: string): boolean {
  const name: string = path.basename(source);
  if (name === '.bin' || name === 'electron-updater') return false;
  if (path.basename(path.dirname(source)) !== '@napi-rs') return true;
  const native: RegExpExecArray | null = /^(?:canvas|keyring)-(linux|win32|darwin)-(x64|arm64)(?:-|$)/.exec(name);
  return !native || (native[1] === platform && native[2] === architecture);
}
