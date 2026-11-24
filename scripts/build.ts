/** Bundle application code; native keyring/locking dependencies remain packaged runtime modules. */
import { build } from 'esbuild';
import { cp, mkdir, readFile, rm } from 'node:fs/promises';
await rm('dist', { recursive: true, force: true });
await mkdir('dist', { recursive: true });
await cp('build/icon.png', 'dist/icon.png');
const { version }: { version: string } = JSON.parse(await readFile('package.json', 'utf8'));
await build({
  entryPoints: { main: 'src/app/bootstrap.ts', cli: 'src/cli.ts', preload: 'src/app/preload.ts', 'smoke-server': 'scripts/smoke-server.ts' },
  outdir: 'dist', outExtension: { '.js': '.cjs' }, bundle: true, platform: 'node', format: 'cjs', target: 'node24',
  define: { 'import.meta.dirname': '__dirname', PROVIDER_ROUTER_VERSION: JSON.stringify(version) },
  external: ['electron', '@napi-rs/keyring', 'fs-native-extensions', 'electron-updater'],
});
await build({ entryPoints: ['src/app/services/provider-service/document-worker.ts'], outfile: 'dist/document-worker.mjs', bundle: true, packages: 'external', platform: 'node', format: 'esm', target: 'node24' });
await build({ entryPoints: ['src/app/products/router/presentation/settings.ts'], outfile: 'dist/renderer/settings.js', bundle: true, platform: 'browser', format: 'iife', target: 'chrome140' });
await cp('src/app/products/router/presentation/index.html', 'dist/renderer/index.html');
await cp('src/app/products/router/presentation/settings.css', 'dist/renderer/settings.css');
await cp('src/app/products/router/presentation/assets', 'dist/renderer/assets', { recursive: true });
