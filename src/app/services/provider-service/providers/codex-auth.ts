/** Reuse Codex subscription credentials without introducing another application login. */
import { createHash } from 'node:crypto';
import { spawn, type ChildProcess } from 'node:child_process';
import { mkdir, open, readFile, realpath, rename, rm, type FileHandle } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { Entry } from '@napi-rs/keyring';
import { tryLock, unlock } from 'fs-native-extensions';
import { z } from 'zod';
import { parse as parseToml } from 'smol-toml';
import { RouterError } from '../../../common/schemas/contracts.ts';
import { uuid7 } from '../../../common/identity/identity.ts';

const credentialsSchema = z.object({
  auth_mode: z.string().optional(), OPENAI_API_KEY: z.string().nullable().optional(),
  tokens: z.object({ access_token: z.string().min(1), refresh_token: z.string().min(1), id_token: z.string().optional(), account_id: z.string().nullable().optional(), login_owner_id: z.string().optional(), oauth_provenance: z.object({ issuer: z.string().url(), client_id: z.string().min(1) }).optional() }).passthrough(),
  last_refresh: z.string().nullable().optional(),
}).passthrough();
export type CodexCredentials = z.infer<typeof credentialsSchema>;
export interface CredentialStore { load(): Promise<CodexCredentials>; save(expected: CodexCredentials, updated: CodexCredentials): Promise<CodexCredentials> }
const grants: Map<string, Promise<void>> = new Map();
const CLIENT_ID: string = 'app_EMoamEEZ73f0CkXaXp7hrann';
const TOKEN_URL: string = 'https://auth.openai.com/oauth/token';

/** Effective CODEX_HOME can be selected per connection; the process default is preserved. */
export function codexHome(override?: string): string { return override || process.env.CODEX_HOME || path.join(os.homedir(), '.codex'); }

export interface CodexAuthSettings { store: 'file' | 'keyring' | 'auto'; secret_auth_storage: boolean }
/** Read only authentication storage choices; never import operational CLI configuration. */
export async function codexAuthSettings(home: string): Promise<CodexAuthSettings> {
  let text: string;
  try { text = await readFile(path.join(home, 'config.toml'), 'utf8'); }
  catch (error: unknown) { if (error instanceof Error && 'code' in error && error.code === 'ENOENT') return { store: 'file', secret_auth_storage: false }; throw new RouterError('CODEX_CONFIG_UNAVAILABLE', 'Codex authentication configuration could not be read.', 400); }
  const parsed = z.object({ cli_auth_credentials_store: z.enum(['file', 'keyring', 'auto']).default('file'), features: z.object({ secret_auth_storage: z.boolean().default(false) }).optional() }).safeParse(parseToml(text));
  if (!parsed.success) throw new RouterError('CODEX_STORAGE_UNSUPPORTED', 'This credential configuration requires a compatible Codex CLI.', 400);
  return { store: parsed.data.cli_auth_credentials_store, secret_auth_storage: parsed.data.features?.secret_auth_storage || false };
}

/** JWT payload metadata is untrusted; only upstream verifies the bearer credential. */
export function tokenClaims(token: string): Record<string, unknown> {
  try { const value: unknown = JSON.parse(Buffer.from(token.split('.')[1] || '', 'base64url').toString()); return typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {}; }
  catch { return {}; }
}

/** Access is refreshed once after a guarded reload; rotating grants are never retried blindly. */
export class CodexAuth {
  private lastAccess: string | undefined;
  constructor(private readonly home: string, private readonly storage: CredentialStore, private readonly fetcher: typeof fetch = fetch, private readonly tokenUrl: string = TOKEN_URL) {}

  async headers(forceRefresh: boolean = false): Promise<Record<string, string>> {
    const prior: Promise<void> = grants.get(this.home) || Promise.resolve();
    let release: () => void = (): void => {};
    const current: Promise<void> = new Promise<void>((resolve): void => { release = resolve; });
    grants.set(this.home, current);
    await prior;
    try {
      let credentials: CodexCredentials = await this.storage.load();
      const access: string = credentials.tokens.access_token;
      const expiration: unknown = tokenClaims(access).exp;
      const expired: boolean = typeof expiration === 'number' ? expiration <= Date.now() / 1000 + 60 : !credentials.last_refresh || Date.now() - Date.parse(credentials.last_refresh) > 7 * 86400_000;
      const changed: boolean = this.lastAccess !== undefined && this.lastAccess !== access;
      if (expired || (forceRefresh && !changed)) credentials = await this.refresh(credentials);
      this.lastAccess = credentials.tokens.access_token;
      const account: string | null | undefined = credentials.tokens.account_id;
      return { Authorization: `Bearer ${credentials.tokens.access_token}`, ...(account ? { 'ChatGPT-Account-ID': account } : {}) };
    } finally { release(); if (grants.get(this.home) === current) grants.delete(this.home); }
  }

  private async refresh(expected: CodexCredentials): Promise<CodexCredentials> {
    // Do not connect caller cancellation to the token exchange: the refresh grant may rotate.
    // Complete its durable commit even when the corresponding inference was cancelled.
    const provenance: CodexCredentials['tokens']['oauth_provenance'] = expected.tokens.oauth_provenance;
    // Never infer an OAuth authority from unverified JWT payloads. A non-default
    // persisted authority must use HTTPS and is kept bound to its original client.
    const target: URL = new URL(provenance ? `${provenance.issuer.replace(/\/$/, '')}/oauth/token` : this.tokenUrl);
    if (target.protocol !== 'https:' && !(this.tokenUrl !== TOKEN_URL && target.hostname === '127.0.0.1')) throw new RouterError('CODEX_AUTHORITY_UNSUPPORTED', 'This Codex OAuth authority requires a secure endpoint.', 400);
    const marker: string = path.join(this.home, 'auth.provider-router-refresh.json');
    const generation: string = createHash('sha256').update(JSON.stringify({ access_token: expected.tokens.access_token, refresh_token: expected.tokens.refresh_token, account_id: expected.tokens.account_id, login_owner_id: expected.tokens.login_owner_id })).digest('hex');
    const release: () => Promise<void> = await credentialLock(this.home);
    try {
      const latest: CodexCredentials = await this.storage.load();
      if (!sameIdentity(expected, latest)) throw new RouterError('CODEX_ACCOUNT_CHANGED', 'The existing Codex account changed before refresh.', 409);
      if (latest.tokens.access_token !== expected.tokens.access_token) return latest;
      let prior: unknown = null;
      try { prior = JSON.parse(await readFile(marker, 'utf8')) as unknown; } catch (error: unknown) { if (!(error instanceof Error) || !('code' in error) || error.code !== 'ENOENT') throw new RouterError('CODEX_REFRESH_UNCERTAIN', 'A previous credential refresh needs manual repair in Codex.', 409); }
      if (typeof prior === 'object' && prior !== null && 'generation' in prior && prior.generation === generation) throw new RouterError('CODEX_REFRESH_UNCERTAIN', 'A previous refresh did not finish. Refresh the login in Codex before retrying.', 409);
      const handle: FileHandle = await open(marker, 'w', 0o600);
      try { await handle.writeFile(JSON.stringify({ generation, started_at: new Date().toISOString() })); await handle.sync(); } finally { await handle.close(); }
    } finally { await release(); }
    const response: Response = await this.fetcher(target, {
      method: 'POST', redirect: 'error', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ grant_type: 'refresh_token', client_id: provenance?.client_id || CLIENT_ID, refresh_token: expected.tokens.refresh_token }), signal: AbortSignal.timeout(30_000),
    });
    if (!response.ok) {
      const reloaded: CodexCredentials = await this.storage.load();
      if (sameIdentity(expected, reloaded) && reloaded.tokens.access_token !== expected.tokens.access_token) return reloaded;
      throw new RouterError('CODEX_AUTH_EXPIRED', 'The existing Codex login could not be refreshed. Reauthenticate with Codex and try again.', 401);
    }
    const parsed: unknown = await response.json();
    const tokens = z.object({ access_token: z.string().min(1), refresh_token: z.string().optional(), id_token: z.string().optional() }).parse(parsed);
    const updated: CodexCredentials = { ...expected, tokens: { ...expected.tokens, ...tokens }, last_refresh: new Date().toISOString() };
    const saved: CodexCredentials = await this.storage.save(expected, updated);
    // Retain the generation marker on any uncertain exchange/commit failure. A
    // subsequent request must not replay a refresh grant that may have rotated.
    const commitRelease: () => Promise<void> = await credentialLock(this.home);
    try { const current: unknown = JSON.parse(await readFile(marker, 'utf8')); if (typeof current === 'object' && current !== null && 'generation' in current && current.generation === generation) await rm(marker, { force: true }); }
    finally { await commitRelease(); }
    return saved;
  }
}

function sameIdentity(left: CodexCredentials, right: CodexCredentials): boolean { return left.tokens.account_id === right.tokens.account_id && left.tokens.login_owner_id === right.tokens.login_owner_id && JSON.stringify(left.tokens.oauth_provenance) === JSON.stringify(right.tokens.oauth_provenance); }

/** Credential writes use Codex's stable lock sidecar and compare the complete old generation. */
export class ExistingCodexCredentials implements CredentialStore {
  constructor(private readonly home: string) {}

  async load(): Promise<CodexCredentials> {
    try {
      const backend: 'file' | 'keyring' | 'auto' = await this.backend();
      let value: unknown;
      if (backend !== 'file') {
        const serialized: string | null = (await this.keyring()).getPassword();
        if (serialized) value = JSON.parse(serialized) as unknown;
        else if (backend === 'keyring') throw new Error('missing');
      }
      if (!value) value = JSON.parse(await readFile(path.join(this.home, 'auth.json'), 'utf8')) as unknown;
      const credentials: CodexCredentials = credentialsSchema.parse(value);
      if (credentials.auth_mode && credentials.auth_mode !== 'chatgpt') throw new Error('not subscription');
      return credentials;
    } catch (error: unknown) {
      if (error instanceof RouterError) throw error;
      throw new RouterError('CODEX_LOGIN_UNAVAILABLE', 'No readable Codex subscription login was found. Use Codex CLI mode if your credentials use an unsupported encrypted backend.', 401);
    }
  }

  async save(expected: CodexCredentials, updated: CodexCredentials): Promise<CodexCredentials> {
    const release: () => Promise<void> = await credentialLock(this.home);
    try {
      const current: CodexCredentials = await this.load();
      if (!sameIdentity(expected, current)) throw new RouterError('CODEX_ACCOUNT_CHANGED', 'The Codex account changed while inference was running.', 409);
      if (JSON.stringify(current) !== JSON.stringify(expected)) {
        if (current.tokens.access_token !== expected.tokens.access_token) return current;
        throw new RouterError('CODEX_CREDENTIALS_CHANGED', 'Codex credentials changed before the refreshed login could be saved.', 409);
      }
      const backend: 'file' | 'keyring' | 'auto' = await this.backend();
      if (backend !== 'file') {
        const entry: Entry = await this.keyring();
        if (backend === 'keyring' || entry.getPassword()) { entry.setPassword(JSON.stringify(updated)); return updated; }
      }
      const destination: string = path.join(this.home, 'auth.json');
      const temporary: string = `${destination}.${uuid7()}.tmp`;
      const handle: FileHandle = await open(temporary, 'wx', 0o600);
      try { await handle.writeFile(JSON.stringify(updated)); await handle.sync(); }
      finally { await handle.close(); }
      try { await rename(temporary, destination); } finally { await rm(temporary, { force: true }); }
      return updated;
    } finally { await release(); }
  }

  private async backend(): Promise<'file' | 'keyring' | 'auto'> {
    const settings: CodexAuthSettings = await codexAuthSettings(this.home);
    if (settings.store !== 'file' && settings.secret_auth_storage) throw new RouterError('CODEX_STORAGE_UNSUPPORTED', 'Codex encrypted secrets storage requires Codex CLI mode.', 400);
    return settings.store;
  }

  private async keyring(): Promise<Entry> {
    const canonical: string = await realpath(this.home).catch((): string => this.home);
    const key: string = `cli|${createHash('sha256').update(canonical).digest('hex').slice(0, 16)}`;
    return new Entry('Codex Auth', key);
  }
}

async function credentialLock(home: string): Promise<() => Promise<void>> {
  await mkdir(home, { recursive: true, mode: 0o700 });
  const file: string = path.join(home, 'auth.json.lock');
  // fs-native-extensions uses BSD flock on macOS and LockFileEx on Windows.
  // Its Linux OFD lock does not interoperate with Codex flock, so the development
  // adapter uses the OS flock utility on Linux instead. Linux installers are not shipped.
  if (process.platform === 'linux') {
    const child: ChildProcess = spawn('flock', ['-x', '-w', '10', file, 'sh', '-c', 'printf ready; cat >/dev/null'], { stdio: ['pipe', 'pipe', 'ignore'] });
    await new Promise<void>((resolve, reject): void => {
      child.once('error', reject); child.once('exit', (code): void => { if (code) reject(new RouterError('CODEX_STORAGE_LOCKED', 'Codex credentials are busy.', 409)); });
      child.stdout?.once('data', (): void => resolve());
    });
    return async (): Promise<void> => { child.stdin?.end(); await new Promise<void>((resolve): void => { child.once('exit', (): void => resolve()); if (child.exitCode !== null) resolve(); }); };
  }
  const handle: FileHandle = await open(file, 'a+', 0o600);
  const deadline: number = Date.now() + 10_000;
  try { while (!tryLock(handle.fd)) { if (Date.now() > deadline) throw new RouterError('CODEX_STORAGE_LOCKED', 'Codex credentials are busy.', 409); await delay(25); } }
  catch (error: unknown) { await handle.close(); throw error; }
  return async (): Promise<void> => { unlock(handle.fd); await handle.close(); };
}
