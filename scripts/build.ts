/** Bundle application code; native keyring/locking dependencies remain packaged runtime modules. */
import { build } from 'esbuild';
import { cp, mkdir, rm } from 'node:fs/promises';
// RevExt: 1
await rm('dist', { recursive: true, force: true });
await mkdir('dist', { recursive: true });
await build({ entryPoints: { main: 'src/app/bootstrap.ts', preload: 'src/app/preload.ts', 'smoke-server': 'scripts/smoke-server.ts' }, outdir: 'dist', outExtension: { '.js': '.cjs' }, bundle: true, platform: 'node', format: 'cjs', target: 'node24', external: ['electron', '@napi-rs/keyring', 'fs-native-extensions', 'electron-updater'] });
await build({ entryPoints: ['src/app/products/router/presentation/settings.ts'], outfile: 'dist/renderer/settings.js', bundle: true, platform: 'browser', format: 'iife', target: 'chrome140' });
await cp('src/app/products/router/presentation/index.html', 'dist/renderer/index.html');
await cp('src/app/products/router/presentation/settings.css', 'dist/renderer/settings.css');
// RevExt: 2
await cp('src/app/products/router/presentation/assets', 'dist/renderer/assets', { recursive: true });
