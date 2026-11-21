/** Expose a fixed settings IPC channel without Node, filesystem or arbitrary HTTP access. */
import { contextBridge, ipcRenderer } from 'electron';
import type { DesktopSettingsBridge, DesktopCommand, SettingsReply } from './products/router/schemas/settings-contract.ts';

const bridge: DesktopSettingsBridge = {
  invoke: (command: DesktopCommand): Promise<SettingsReply> => ipcRenderer.invoke('provider-router:settings', command) as Promise<SettingsReply>,
  onShow: (refresh: () => void): void => { ipcRenderer.on('provider-router:settings-shown', (): void => refresh()); },
};
contextBridge.exposeInMainWorld('providerRouter', bridge);
