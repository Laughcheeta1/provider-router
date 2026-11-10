/** Authenticate renderer IPC senders before invoking product settings workflows. */
import type { BrowserWindow, IpcMainInvokeEvent } from 'electron';
import { pathToFileURL } from 'node:url';
import { RouterError } from '../../common/schemas/contracts.ts';
import { commandSchema } from './schemas/settings-command.ts';
import type { SettingsCommand, SettingsReply } from './schemas/settings-contract.ts';
import type { ManageUseCase } from './use-cases/manage/use-case.ts';
export function createSettingsHandler(getWindow: () => BrowserWindow | null, renderer: string, useCase: ManageUseCase): (event: IpcMainInvokeEvent, raw: unknown) => Promise<SettingsReply> {
  return async (event: IpcMainInvokeEvent, raw: unknown): Promise<SettingsReply> => {
    const window: BrowserWindow | null = getWindow();
    const frame: URL = new URL(event.senderFrame?.url || 'about:blank'); frame.hash = '';
    if (!window || event.sender !== window.webContents || event.senderFrame !== window.webContents.mainFrame || frame.href !== pathToFileURL(renderer).href) return { ok: false, error: 'This settings request was rejected.' };
    try {
      const command: SettingsCommand = commandSchema.parse(raw);
      return await useCase.execute(command);
    } catch (error: unknown) { return { ok: false, error: error instanceof RouterError ? error.message : 'This operation could not finish. Check the settings and provider connection.' }; }
  };
}
