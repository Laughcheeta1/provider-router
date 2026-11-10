/** Private local configuration and encrypted upstream credentials. */
import { createHash, randomBytes, timingSafeEqual } from 'node:crypto';
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { z } from 'zod';
import { connectionSchema, RouterError, uuidSchema, type Connection } from '../../../../common/schemas/contracts.ts';
import { uuid7 } from '../../../../common/identity/identity.ts';

import { networkSchema, stateSchema, type State, type Client, type NetworkSettings, type PublicState } from '../../schemas/state.ts';
import type { RouterRepository } from './contract.ts';
export { networkSchema, type Client, type NetworkSettings, type PublicState } from '../../schemas/state.ts';
export interface SecretCipher { encrypt(value: string): Buffer; decrypt(value: Buffer): string }

/** Atomic persistence remains owned by the main process; renderers see safe projections only. */
export class RouterStore implements RouterRepository {
  private state: State;
  private writes: Promise<void> = Promise.resolve();
  private constructor(private readonly file: string, private readonly cipher: SecretCipher, state: State) { this.state = state; }

  static async open(directory: string, cipher: SecretCipher): Promise<RouterStore> {
    const file: string = path.join(directory, 'settings.json');
    let state: State;
    try {
      const stored: unknown = JSON.parse(await readFile(file, 'utf8'));
      // Existing installations retain their grants instead of becoming unauthenticated on upgrade.
      if (stored && typeof stored === 'object' && 'network' in stored && stored.network && typeof stored.network === 'object' && !('require_api_key' in stored.network)) Object.assign(stored.network, { require_api_key: true });
      state = stateSchema.parse(stored);
    }  // RevExt: 1
    catch (error: unknown) {
      if (!(error instanceof Error) || !('code' in error) || error.code !== 'ENOENT') throw new RouterError('SETTINGS_INVALID', 'The local configuration could not be read. Restore or repair it before starting.', 500);
      state = { version: 1, instance_id: uuid7(), connections: [], clients: [], secrets: {}, network: networkSchema.parse({}) };
    }  // RevExt: 2
    return new RouterStore(file, cipher, state);
  }

  authenticate(token: string): Client | undefined {
    const hash: Buffer = createHash('sha256').update(token).digest();
    return this.state.clients.find((client: Client): boolean => {
      const expected: Buffer = Buffer.from(client.token_hash, 'hex');
      return expected.length === hash.length && timingSafeEqual(expected, hash);
    });
  }

  async createClient(name: string): Promise<{ id: string; token: string }> {
    const parsed: string = z.string().trim().min(1).max(100).parse(name);
    const token: string = randomBytes(32).toString('base64url');
    const id: string = uuid7();
    await this.update((state: State): void => { state.clients.push({ id, name: parsed, token_hash: createHash('sha256').update(token).digest('hex'), created_at: new Date().toISOString() }); });
    return { id, token };
  }

  connection(id: string): Connection | undefined { return this.state.connections.find((item: Connection): boolean => item.id === id); }
  connections(): Connection[] { return structuredClone(this.state.connections); }

  publicState(): PublicState {
    return { instance_id: this.state.instance_id, connections: this.connections(), network: structuredClone(this.state.network), clients: this.state.clients.map(({ token_hash: _hash, ...client }: Client): Omit<Client, 'token_hash'> => client) };
  }

  async removeConnection(id: string): Promise<void> {
    await this.update((state: State): void => { state.connections = state.connections.filter((item: Connection): boolean => item.id !== id); delete state.secrets[id]; });
  }

  async revokeClient(id: string): Promise<void> {
    await this.update((state: State): void => { state.clients = state.clients.filter((client: Client): boolean => client.id !== id); });
  }

  async saveConnection(input: unknown, apiKey?: string): Promise<void> {
    const connection: Connection = connectionSchema.parse(input);
    const encrypted: string | undefined = apiKey ? this.cipher.encrypt(apiKey).toString('base64') : undefined;
    await this.update((state: State): void => {
      state.connections = [...state.connections.filter((item: Connection): boolean => item.id !== connection.id), connection];
      if (encrypted) state.secrets[connection.id] = encrypted;
    });
  }

  async saveNetwork(input: unknown): Promise<void> { const network: NetworkSettings = networkSchema.parse(input); await this.update((state: State): void => { state.network = network; }); }
  secret(id: string): string | undefined { const value: string | undefined = this.state.secrets[id]; return value ? this.cipher.decrypt(Buffer.from(value, 'base64')) : undefined; }

  private async update(change: (state: State) => void): Promise<void> {
    const write: () => Promise<void> = async (): Promise<void> => {
      const next: State = structuredClone(this.state);
      change(next);
      const content: string = JSON.stringify(next);
      await mkdir(path.dirname(this.file), { recursive: true, mode: 0o700 });
      const temporary: string = `${this.file}.${uuid7()}.tmp`;
      await writeFile(temporary, content, { mode: 0o600 });
      await rename(temporary, this.file);
      this.state = next;
    };
    this.writes = this.writes.catch((): void => {}).then(write);
    await this.writes;
  }
}
