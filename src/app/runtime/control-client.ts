/** Send settings commands to the owning process, or work offline under the same profile lock. */
import { spawn, type ChildProcess } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { request, type ClientRequest, type IncomingMessage } from 'node:http';
import { setTimeout as delay } from 'node:timers/promises';
import { RouterError } from '../common/schemas/contracts.ts';
import { commandSchema } from '../products/router/schemas/settings-command.ts';
import type { SettingsBridge, SettingsCommand, SettingsReply } from '../products/router/schemas/settings-contract.ts';
import { launchArgs, launchEnvironment, type LaunchCommand } from './launch.ts';
import { lockProfile, type RouterProfile } from './profile.ts';
import { RouterRuntime } from './router-runtime.ts';

export class ControlClient implements SettingsBridge {
  constructor(
    private readonly profile: RouterProfile,
    private readonly launch: LaunchCommand,
    private readonly version: string,
  ) {}

  async attach(signal: AbortSignal): Promise<void> {
    const response: IncomingMessage = await this.connect('GET', '/view', undefined, signal);
    response.on('error', (): void => {});
    response.resume();
  }

  async invoke(command: SettingsCommand): Promise<SettingsReply> {
    const validated: ReturnType<typeof commandSchema.safeParse> = commandSchema.safeParse(command);
    if (!validated.success) return { ok: false, error: `Invalid ${validated.error.issues[0]?.path.join('.') || 'settings command'}.` };
    const parsed: SettingsCommand = validated.data;
    for (let attempt: number = 0; attempt < 100; attempt++) {
      const reply: SettingsReply | undefined = await this.online(parsed);
      if (reply) return reply;
      const release: (() => Promise<void>) | undefined = await lockProfile(this.profile);
      if (release) {
        try {
          const runtime: RouterRuntime = await RouterRuntime.open(this.profile, this.launch, this.version, false);
          return await runtime.execute(parsed);
        } finally {
          await release();
        }
      }

      await delay(50);
    }

    throw new RouterError('ROUTER_BUSY', 'Provider Router is busy. Try again.');
  }

  async start(): Promise<SettingsReply> {
    const existing: SettingsReply | undefined = await this.online({ type: 'status' });
    if (existing) return existing;

    const child: ChildProcess = spawn(this.launch.executable, launchArgs(this.launch, this.profile.directory, 'background'), {
      detached: true, env: launchEnvironment(this.launch), stdio: 'ignore', windowsHide: true,
    });
    let failed: boolean = false;
    child.once('error', (): void => { failed = true; });
    child.unref();
    for (let attempt: number = 0; attempt < 100 && !failed; attempt++) {
      const result: SettingsReply | undefined = await this.online({ type: 'status' });
      if (result) return result;
      await delay(50);
    }

    // Read the settings offline to surface a corrupt configuration on startup.
    await this.invoke({ type: 'status' });
    throw new RouterError('START_FAILED', 'Provider Router could not open its local control channel. Check the data directory or run provider-router serve for details.');
  }

  async stop(): Promise<void> {
    try {
      const token: string = await readFile(this.profile.tokenFile, 'utf8');
      const response: IncomingMessage = await this.connect('POST', '/stop');
      response.resume();
      for (let attempt: number = 0; attempt < 100; attempt++) {
        if (await readFile(this.profile.tokenFile, 'utf8') !== token) return;
        await delay(50);
      }
      throw new RouterError('STOP_PENDING', 'Provider Router is still shutting down. Check its status in a moment.');
    } catch (error: unknown) {
      if (!isUnavailable(error)) throw error;
    }
  }

  private async connect(method: string, route: string, payload?: string, signal?: AbortSignal): Promise<IncomingMessage> {
    const token: string = await readFile(this.profile.tokenFile, 'utf8');
    return new Promise<IncomingMessage>((resolve, reject): void => {
      const outgoing: ClientRequest = request({
        socketPath: this.profile.socket, path: route, method, signal, agent: false,
        headers: { authorization: token, ...(payload ? { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) } : {}) },
      }, (response: IncomingMessage): void => {
        if (response.statusCode === 200) resolve(response);
        else if (response.statusCode === 503) {
          response.resume();
          reject(new RouterError('CONTROL_STOPPING', 'The local router is shutting down.'));
        } else {
          response.resume();
          reject(new RouterError('CONTROL_UNAVAILABLE', 'The local router could not accept this settings request.'));
        }
      });
      if (route !== '/view') outgoing.setTimeout(45_000, (): void => { outgoing.destroy(new RouterError('CONTROL_TIMEOUT', 'The local router did not finish the settings request.')); });
      outgoing.once('error', reject);
      outgoing.end(payload);
    });
  }

  private async online(command: SettingsCommand): Promise<SettingsReply | undefined> {
    let response: IncomingMessage;
    try {
      response = await this.connect('POST', '/settings', JSON.stringify(command));
    } catch (error: unknown) {
      if (isUnavailable(error)) return undefined;
      throw error;
    }

    const chunks: Buffer[] = [];
    for await (const chunk of response) chunks.push(Buffer.from(chunk));
    return JSON.parse(Buffer.concat(chunks).toString('utf8')) as SettingsReply;
  }
}

function isUnavailable(error: unknown): boolean {
  return error instanceof Error && 'code' in error && (error.code === 'ENOENT' || error.code === 'ECONNREFUSED' || error.code === 'CONTROL_STOPPING');
}
