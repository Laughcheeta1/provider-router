/** Product-owned ports for persisted configuration and client authorization. */
import type { Connection } from '../../../../common/schemas/contracts.ts';
import type { Client, NetworkSettings, PublicState } from '../../schemas/state.ts';
export interface RouterRepository {
  authenticate(token: string): Client | undefined;
  connection(id: string): Connection | undefined;
  connections(): Connection[];
  createClient(name: string): Promise<{ id: string; token: string }>;
  publicState(): PublicState;
  removeConnection(id: string): Promise<void>;
  revokeClient(id: string): Promise<void>;
  saveConnection(input: unknown, apiKey?: string): Promise<void>;
  saveNetwork(input: unknown): Promise<void>;
}
