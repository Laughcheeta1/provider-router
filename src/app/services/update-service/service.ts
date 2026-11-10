/** Explicit desktop update checks and user-approved installation. */
import { app, dialog, shell } from 'electron';
import updater from 'electron-updater';
import { RouterError } from '../../common/schemas/contracts.ts';
export async function checkUpdates(): Promise<void> {
    if (!app.isPackaged) throw new RouterError('UPDATES_UNAVAILABLE', 'Update checks are available in installed releases.');
    if (process.platform === 'darwin') {
      // Unsigned macOS releases use the download flow until signing is configured.
      const download = await dialog.showMessageBox({ message: 'Download the latest Provider Router release.', buttons: ['Open downloads', 'Cancel'], cancelId: 1 });
      if (download.response === 0) await shell.openExternal('https://github.com/Laughcheeta1/provider-router/releases/latest');
      return;
    }
    const { autoUpdater }: typeof updater = updater;
    autoUpdater.autoDownload = false;
    const result = await autoUpdater.checkForUpdates();
    if (!result?.isUpdateAvailable) { await dialog.showMessageBox({ message: 'Provider Router is up to date.' }); return; }
    const choice = await dialog.showMessageBox({ message: `Provider Router ${result.updateInfo.version} is available.`, buttons: ['Download', 'Later'], cancelId: 1 });
    if (choice.response === 0) { await autoUpdater.downloadUpdate(); const install = await dialog.showMessageBox({ message: 'The update is ready.', buttons: ['Install and restart', 'Later'], cancelId: 1 }); if (install.response === 0) autoUpdater.quitAndInstall(); }
    return;
}
