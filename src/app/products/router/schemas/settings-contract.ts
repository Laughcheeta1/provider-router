/** Narrow settings surface exposed by the sandboxed renderer preload. */
import type { Connection, Model } from '../../../common/schemas/contracts.ts';
import type { PublicState } from './state.ts';

export type SettingsCommand =
  | { type: 'snapshot' }
  | { type: 'save_connection'; connection: Connection; api_key?: string }
  | { type: 'remove_connection'; id: string }
  | { type: 'refresh_models'; id: string }
  | { type: 'create_client'; name: string }
  | { type: 'revoke_client'; id: string }
  | { type: 'save_network'; network: PublicState['network'] }
  | { type: 'check_updates' };
export interface SettingsSnapshot extends PublicState { endpoint: string | null; error: string | null; wsl_distributions: string[]; platform: NodeJS.Platform }
export type SettingsReply = { ok: true; value: SettingsSnapshot | Model[] | { id: string; token: string } | null } | { ok: false; error: string };
export interface SettingsBridge { invoke(command: SettingsCommand): Promise<SettingsReply> }
