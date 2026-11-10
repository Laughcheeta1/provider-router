/** Provider Router owns its window, credential vault, API lifecycle and standalone updates. */
import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, safeStorage, Tray, type MenuItemConstructorOptions } from 'electron';
import { mkdir, rename, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import type { ManageUseCase } from './products/router/use-cases/manage/use-case.ts';
import { buildManageUseCase } from './products/router/dependencies.ts';
import { createSettingsHandler } from './products/router/settings-router.ts';
import { checkUpdates } from './services/update-service/service.ts';
import { CodexCliAdapter } from './services/provider-service/providers/codex-cli.ts';
import { CodexHttpAdapter, OpenRouterAdapter } from './services/provider-service/providers/http-providers.ts';
import { RouterError, type Method, type ProviderAdapter } from './common/schemas/contracts.ts';
import { uuid7 } from './common/identity/identity.ts';
import { startRouter, type ListeningRouter } from './products/router/router.ts';
import { RouterStore, type SecretCipher } from './products/router/repository/settings/file.ts';
import { listWslDistributions } from './services/provider-service/providers/wsl-codex.ts';

app.setName('Provider Router');
app.setAppUserModelId('org.providerrouter.desktop');
const dataDirectory: string = !app.isPackaged && process.env.PROVIDER_ROUTER_DATA_DIR ? process.env.PROVIDER_ROUTER_DATA_DIR : path.join(app.getPath('appData'), 'provider-router');
app.setPath('userData', dataDirectory);
let window: BrowserWindow | null = null;
let tray: Tray | null = null;
let listener: ListeningRouter | null = null;
let startupError: string | null = null;
let store: RouterStore;
let adapters: Record<Method, ProviderAdapter>;
let quitting: boolean = false;
const renderer: string = path.join(app.getAppPath(), 'dist', 'renderer', 'index.html');
/** Closing settings keeps the independent API available to every authorized client. */
async function showSettings(): Promise<void> {
  if (window) { window.show(); window.focus(); return; }
  window = new BrowserWindow({ title: 'Provider Router', width: 1050, height: 820, minWidth: 720, minHeight: 600, backgroundColor: '#000000', webPreferences: { preload: path.join(app.getAppPath(), 'dist', 'preload.cjs'), contextIsolation: true, sandbox: true, nodeIntegration: false, webviewTag: false } });
  window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  window.webContents.on('will-navigate', (event): void => event.preventDefault());
  window.webContents.session.setPermissionRequestHandler((_contents, _permission, callback): void => callback(false));
  window.webContents.session.setPermissionCheckHandler((): boolean => false);
  window.on('close', (event): void => { if (!quitting) { event.preventDefault(); window?.hide(); } });
  window.on('closed', (): void => { window = null; });
  await window.loadFile(renderer);
}  // RevExt: 1

async function restartListener(): Promise<void> {
  await listener?.close(); listener = null;
  await rm(path.join(dataDirectory, 'discovery.json'), { force: true });
  try {
    listener = await startRouter(store, adapters, store.publicState().network, app.getVersion()); startupError = null;
    const descriptor: Record<string, unknown> = { endpoint: listener.endpoint, protocol_version: 1, instance_id: store.publicState().instance_id, pid: process.pid, version: app.getVersion() };
    const temporary: string = path.join(dataDirectory, `discovery-${uuid7()}.tmp`);
    await writeFile(temporary, JSON.stringify(descriptor), { mode: 0o600 });
    await rename(temporary, path.join(dataDirectory, 'discovery.json'));
  } catch { startupError = 'The API could not start. Check the listener port, remote access and certificate settings.'; }
}  // RevExt: 2


if (!app.requestSingleInstanceLock()) app.quit();
else {
  app.on('second-instance', (): void => { void showSettings(); });
  app.on('open-url', (event): void => { event.preventDefault(); if (app.isReady()) void showSettings(); });
  app.on('activate', (): void => { void showSettings(); });
  app.on('window-all-closed', (): void => { /* The independent API continues running in the tray. */ });
  app.on('before-quit', (event): void => {
    if (quitting) return;
    event.preventDefault(); quitting = true;
    void Promise.all([listener?.close(), rm(path.join(dataDirectory, 'discovery.json'), { force: true })]).finally((): void => { tray?.destroy(); app.quit(); });
  });
  void app.whenReady().then(async (): Promise<void> => {
    await mkdir(dataDirectory, { recursive: true, mode: 0o700 });
    const cipher: SecretCipher = {
      encrypt: (value: string): Buffer => { if (!safeStorage.isEncryptionAvailable() || (process.platform === 'linux' && safeStorage.getSelectedStorageBackend() === 'basic_text')) throw new RouterError('VAULT_UNAVAILABLE', 'The operating system credential vault is unavailable.'); return safeStorage.encryptString(value); },
      decrypt: (value: Buffer): string => { if (!safeStorage.isEncryptionAvailable() || (process.platform === 'linux' && safeStorage.getSelectedStorageBackend() === 'basic_text')) throw new RouterError('VAULT_UNAVAILABLE', 'The operating system credential vault is unavailable.'); return safeStorage.decryptString(value); },
    };
    store = await RouterStore.open(dataDirectory, cipher);
    adapters = { codex_cli: new CodexCliAdapter(), codex_http: new CodexHttpAdapter(), openrouter_http: new OpenRouterAdapter((id: string): string | undefined => store.secret(id)) };
    const settingsUseCase: ManageUseCase = buildManageUseCase(store, adapters, {
      endpoint: (): string | null => listener?.endpoint || null, error: (): string | null => startupError,
      distributions: listWslDistributions, platform: process.platform,
      revokeClient: async (id: string): Promise<void> => { if (listener) await listener.revokeClient(id); else await store.revokeClient(id); },
      restart: restartListener, checkUpdates,
    });
    ipcMain.handle('provider-router:settings', createSettingsHandler((): BrowserWindow | null => window, renderer, settingsUseCase));
    app.setAsDefaultProtocolClient('provider-router');
    const menu: Electron.Menu = Menu.buildFromTemplate([{ label: 'Provider Router Settings', click: (): void => { void showSettings(); } }, { type: 'separator' }, { role: 'quit' }]);
    const applicationMenu: MenuItemConstructorOptions[] = [
      ...(process.platform === 'darwin' ? [{ role: 'appMenu' } as MenuItemConstructorOptions] : []),
      { label: 'File', submenu: [{ label: 'Settings…', accelerator: 'CmdOrCtrl+,', click: (): void => { void showSettings(); } }, { type: 'separator' }, { role: process.platform === 'darwin' ? 'close' : 'quit' }] },
      { role: 'editMenu' },
      { label: 'View', submenu: [{ role: 'resetZoom' }, { role: 'zoomIn' }, { role: 'zoomOut' }, { type: 'separator' }, { role: 'togglefullscreen' }] },
      { role: 'windowMenu' },
    ];
    Menu.setApplicationMenu(Menu.buildFromTemplate(applicationMenu));
    tray = new Tray(nativeImage.createFromDataURL('data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAYAAAAf8/9hAAAAIUlEQVR42mNgGAWjYBSMgmEAjIyM/4nBqGAUjIJRMAoGAQAAu4cCCaMqSZsAAAAASUVORK5CYII='));
    tray.setToolTip('Provider Router'); tray.setContextMenu(menu); tray.on('click', (): void => { void showSettings(); });
    await restartListener();
    if (!process.argv.includes('--background') || startupError) await showSettings();
  }).catch((): void => { dialog.showErrorBox('Provider Router could not start', 'The local configuration could not be opened. Repair it before restarting.'); app.quit(); });
}  // RevExt: 3
