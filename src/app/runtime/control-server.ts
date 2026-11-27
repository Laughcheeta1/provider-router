/** Manage the local settings control channel and count live desktop/terminal sessions. */
import { parseNativeJson } from '../common/schemas/native-json.ts';
import { stringifyWireJson } from '../common/schemas/json-wire.ts';
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { rm, writeFile } from 'node:fs/promises';
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import { RouterError } from '../common/schemas/contracts.ts';
import { commandSchema } from '../products/router/schemas/settings-command.ts';
import type { SettingsCommand, SettingsReply } from '../products/router/schemas/settings-contract.ts';
import type { RouterProfile } from './profile.ts';
import type { RouterRuntime } from './router-runtime.ts';

type ServeMode = 'background' | 'boot' | 'foreground';
const maxCommandBytes: number = 2 * 1024 * 1024;

export async function serveControl(profile: RouterProfile, runtime: RouterRuntime, mode: ServeMode, signal: AbortSignal): Promise<void> {
  if (mode === 'boot' && runtime.startupMode() !== 'startup') return;
  const token: Buffer = Buffer.from(randomBytes(32).toString('hex'));
  const views: Set<ServerResponse> = new Set();
  const pending: Set<Promise<void>> = new Set();
  let timer: NodeJS.Timeout | undefined;
  let finish!: () => void;
  const finished: Promise<void> = new Promise((resolve): void => { finish = resolve; });
  let stopping: boolean = false;

  function stop(): void {
    stopping = true;
    finish();
  }

  function scheduleIdleStop(grace: number): void {
    clearTimeout(timer);
    if ((mode === 'background' || mode === 'boot') && runtime.startupMode() === 'while-open' && views.size === 0) {
      timer = setTimeout(stop, grace);
    }
  }

  const server: Server = createServer((request: IncomingMessage, response: ServerResponse): void => {
    const authorization: Buffer = Buffer.from(request.headers.authorization ?? '');
    if (authorization.length !== token.length || !timingSafeEqual(authorization, token)) { response.writeHead(403).end(); return; }
    if (stopping) { response.writeHead(503).end(); return; }

    if (request.method === 'GET' && request.url === '/view') {
      views.add(response);
      clearTimeout(timer);
      response.writeHead(200, { 'Content-Type': 'application/octet-stream' });
      response.flushHeaders();
      response.once('close', (): void => { views.delete(response); if (!stopping) scheduleIdleStop(250); });
      return;
    }

    if (request.method === 'POST' && request.url === '/stop') {
      response.once('finish', stop);
      response.writeHead(200).end();
      return;
    }

    if (request.method === 'POST' && request.url === '/settings') {
      const operation: Promise<void> = handleSettings(request, response, runtime, scheduleIdleStop);
      pending.add(operation);
      void operation.then((): void => { pending.delete(operation); });
      return;
    }

    response.writeHead(404).end();
  });

  let listening: boolean = false;
  try {
    if (process.platform !== 'win32') await rm(profile.socket, { force: true });
    await rm(profile.tokenFile, { force: true });
    await writeFile(profile.tokenFile, token, { mode: 0o600, flag: 'wx' });
    await runtime.restart();
    if (mode === 'foreground' || mode === 'boot') {
      const reply: SettingsReply = await runtime.execute({ type: 'status' });
      if (reply.ok && reply.value && 'endpoint' in reply.value) {
        if (!reply.value.endpoint) throw new RouterError('LISTENER_UNAVAILABLE', reply.value.error ?? 'The API could not start.');
        console.log(`Provider Router is listening at ${reply.value.endpoint}`);
      }
    }

    await new Promise<void>((resolve, reject): void => {
      server.once('error', reject);
      server.listen(profile.socket, (): void => { server.removeListener('error', reject); resolve(); });
    });
    listening = true;
    scheduleIdleStop(5000);
    if (signal.aborted) stop();
    else signal.addEventListener('abort', stop, { once: true });
    await finished;
  } finally {
    stopping = true;
    signal.removeEventListener('abort', stop);
    clearTimeout(timer);
    if (listening) {
      for (const response of views) response.destroy();
      await new Promise<void>((resolve): void => { server.close((): void => resolve()); server.closeAllConnections(); });
    }

    try {
      await Promise.all(pending);
      await runtime.close();
    } finally {
      await rm(profile.tokenFile, { force: true });
      if (process.platform !== 'win32') await rm(profile.socket, { force: true });
    }
  }
}

async function handleSettings(request: IncomingMessage, response: ServerResponse, runtime: RouterRuntime, scheduleIdleStop: (grace: number) => void): Promise<void> {
  try {
    const chunks: Buffer[] = [];
    let length: number = 0;
    for await (const chunk of request) {
      const buffer: Buffer = Buffer.from(chunk);
      length += buffer.length;
      if (length > maxCommandBytes) { response.writeHead(413).end(); return; }
      chunks.push(buffer);
    }

    const raw: unknown = parseNativeJson(Buffer.concat(chunks));
    const command: SettingsCommand = commandSchema.parse(raw);
    const reply: SettingsReply = await runtime.execute(command);
    if (!response.destroyed) response.writeHead(200, { 'Content-Type': 'application/json' }).end(stringifyWireJson(reply));
    if (command.type === 'save_startup' && reply.ok) scheduleIdleStop(250);
  } catch {
    if (!response.destroyed) response.writeHead(400, { 'Content-Type': 'application/json' }).end(JSON.stringify({ ok: false, error: 'Invalid settings command.' }));
  }
}
