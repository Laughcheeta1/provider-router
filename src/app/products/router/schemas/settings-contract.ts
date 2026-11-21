/** Narrow settings surface exposed by the sandboxed renderer preload. */
import type { Connection, DetectedProvider, Model } from '../../../common/schemas/contracts.ts';
import type { PublicState, StartupMode } from './state.ts';

export type SettingsCommand =
  | { type: 'snapshot' }
  | { type: 'status' }
  | { type: 'save_connection'; connection: Connection; api_key?: string }
  | { type: 'remove_connection'; id: string }
  | { type: 'refresh_models'; id: string }
  | { type: 'create_client'; name: string }
  | { type: 'revoke_client'; id: string }
  | { type: 'save_network'; network: PublicState['network'] }
  | { type: 'save_startup'; mode: StartupMode }
  | { type: 'check_updates' };
export type DesktopCommand = SettingsCommand | { type: 'start_api' } | { type: 'stop_api' };
export interface RouterStatus extends PublicState { endpoint: string | null; error: string | null }
export interface SettingsSnapshot extends RouterStatus { wsl_distributions: string[]; platform: NodeJS.Platform; detected_providers: DetectedProvider[] }
export type SettingsReply = { ok: true; value: RouterStatus | SettingsSnapshot | Model[] | { id: string; token: string } | null } | { ok: false; error: string };
export interface SettingsBridge { invoke(command: SettingsCommand): Promise<SettingsReply> }
export interface DesktopSettingsBridge {
  invoke(command: DesktopCommand): Promise<SettingsReply>;
  onShow(refresh: () => void): void;
}
