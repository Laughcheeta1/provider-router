/** Connect the desktop window and tray to the shared Router runtime. */
import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, safeStorage, Tray, type MenuItemConstructorOptions } from 'electron';
import path from 'node:path';
import { createSettingsHandler } from './products/router/settings-router.ts';
import type { DesktopCommand, SettingsReply } from './products/router/schemas/settings-contract.ts';
import { checkUpdates } from './services/update-service/service.ts';
import { RouterError } from './common/schemas/contracts.ts';
import { RouterStore, type SecretCipher } from './products/router/repository/settings/file.ts';
import { ControlClient } from './runtime/control-client.ts';
import { createCredentialVault, isLegacySecret } from './runtime/credential-vault.ts';
import type { LaunchCommand } from './runtime/launch.ts';
import { defaultDataDirectory, lockProfile, openProfile, type RouterProfile } from './runtime/profile.ts';
app.setName('Provider Router');
app.setAppUserModelId('org.providerrouter.desktop');
const background: boolean = process.argv.includes('--background');
const profileArgument: number = process.argv.indexOf('--router-data-dir');
const dataDirectory: string = background && profileArgument !== -1 ? process.argv[profileArgument + 1] ?? defaultDataDirectory() :
  !app.isPackaged && process.env.PROVIDER_ROUTER_DATA_DIR ? process.env.PROVIDER_ROUTER_DATA_DIR : defaultDataDirectory();
app.setPath('userData', dataDirectory);
let window: BrowserWindow | null = null;
let tray: Tray | null = null;
let control: ControlClient | undefined;
let view: AbortController | undefined;
let quitting: boolean = false;
const iconPath: string = path.join(app.getAppPath(), 'dist', 'icon.png');
const renderer: string = path.join(app.getAppPath(), 'dist', 'renderer', 'index.html');
const launch: LaunchCommand = { executable: process.execPath, script: path.join(app.getAppPath(), 'dist', 'cli.cjs'), electron: true };
/** A visible settings window counts as one open application session. */
async function showSettings(): Promise<void> {
  if (window) {
    window.show();
    window.focus();
    await attachView();
    window?.webContents.send('provider-router:settings-shown');
    return;
  }
  window = new BrowserWindow({ icon: iconPath, title: 'Provider Router', width: 1050, height: 820, minWidth: 720, minHeight: 600, backgroundColor: '#000000', webPreferences: { preload: path.join(app.getAppPath(), 'dist', 'preload.cjs'), contextIsolation: true, sandbox: true, nodeIntegration: false, webviewTag: false } });
  window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  window.webContents.on('will-navigate', (event): void => event.preventDefault());
  window.webContents.session.setPermissionRequestHandler((_contents, _permission, callback): void => callback(false));
  window.webContents.session.setPermissionCheckHandler((): boolean => false);
  window.on('close', (event): void => {
    if (!quitting) { event.preventDefault(); window?.hide(); releaseView(); }
  });
  window.on('closed', (): void => { window = null; });
  await attachView();
  if (!quitting) await window?.loadFile(renderer);
}
async function attachView(): Promise<void> {
  if (view || !control) return;
  const session: AbortController = new AbortController();
  view = session;
  try {
    await control.start();
    if (!session.signal.aborted) await control.attach(session.signal);
  } catch {
    if (view === session) view = undefined;
  }
}
if (!app.requestSingleInstanceLock()) app.quit();
else {
  app.on('second-instance', (): void => { if (control) void showSettings(); });
  app.on('open-url', (event): void => { event.preventDefault(); if (control) void showSettings(); });
  app.on('activate', (): void => { if (control) void showSettings(); });
  app.on('window-all-closed', releaseView);
  app.on('before-quit', (event): void => {
    if (quitting) return;
    event.preventDefault();
    quitting = true;
    releaseView();
    tray?.destroy();
    app.quit();
  });
  void app.whenReady().then(async (): Promise<void> => {
    const profile: RouterProfile = await openProfile(dataDirectory);
    await migrateCredentials(profile);
    control = new ControlClient(profile, launch, app.getVersion());
    ipcMain.handle('provider-router:settings', createSettingsHandler((): BrowserWindow | null => window, renderer, { invoke: handleCommand }));
    app.setAsDefaultProtocolClient('provider-router');
    // Tray menus have no GTK accelerator group; avoid the Quit role's implicit shortcut.
    const menu: Electron.Menu = Menu.buildFromTemplate([{ label: 'Provider Router Settings', click: (): void => { void showSettings(); } }, { type: 'separator' }, { label: 'Quit', click: (): void => app.quit() }]);
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
    if (background) {
      const reply: SettingsReply = await control.invoke({ type: 'status' });
      if (reply.ok && reply.value && 'startup_mode' in reply.value && reply.value.startup_mode === 'startup') await control.start();
      app.quit();
    } else {
      await showSettings();
    }
  }).catch((): void => { dialog.showErrorBox('Provider Router could not start', 'The local configuration could not be opened. Repair it before restarting.'); app.quit(); });
}

function releaseView(): void {
  view?.abort();
  view = undefined;
}

async function handleCommand(command: DesktopCommand): Promise<SettingsReply> {
  if (!control) return { ok: false, error: 'Provider Router is still starting.' };
  if (command.type === 'check_updates') {
    const active: ControlClient = control;
    await checkUpdates((): Promise<void> => active.stop());
    return { ok: true, value: null };
  }
  if (command.type === 'start_api') { releaseView(); await attachView(); return control.invoke({ type: 'status' }); }
  if (command.type === 'stop_api') {
    releaseView();
    await control.stop();
    return { ok: true, value: null };
  }

  return control.invoke(command);
}

/** Electron can unlock legacy safeStorage ciphertext during the first desktop startup. */
async function migrateCredentials(profile: RouterProfile): Promise<void> {
  const release: (() => Promise<void>) | undefined = await lockProfile(profile);
  if (!release) return;
  try {
    const store: RouterStore = await RouterStore.open(profile.directory, createCredentialVault(profile.directory));
    const legacy: SecretCipher = {
      encrypt: (value: string): Buffer => safeStorage.encryptString(value),
      decrypt(value: Buffer): string {
        if (!safeStorage.isEncryptionAvailable() || (process.platform === 'linux' && safeStorage.getSelectedStorageBackend() === 'basic_text')) {
          throw new RouterError('VAULT_UNAVAILABLE', 'The operating system credential vault is unavailable.');
        }
        return safeStorage.decryptString(value);
      },
    };
    // A temporarily locked legacy vault must still let users repair the connection.
    await store.migrateSecrets(isLegacySecret, legacy).catch((): void => {});
  } finally {
    await release();
  }
}
