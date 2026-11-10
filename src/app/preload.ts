/** Expose a fixed settings IPC channel without Node, filesystem or arbitrary HTTP access. */
import { contextBridge, ipcRenderer } from 'electron';
import type { SettingsBridge, SettingsCommand, SettingsReply } from './products/router/schemas/settings-contract.ts';

const bridge: SettingsBridge = { invoke: (command: SettingsCommand): Promise<SettingsReply> => ipcRenderer.invoke('provider-router:settings', command) as Promise<SettingsReply> };
contextBridge.exposeInMainWorld('providerRouter', bridge);
