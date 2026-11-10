/** Stateless Codex CLI inference with application tools represented as output data. */
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { mkdir, mkdtemp, open, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { z } from 'zod';
import { RouterError, type Connection, type InferenceRequest, type InferenceResult, type Model, type ProviderAdapter, type ToolCall } from '../../../common/schemas/contracts.ts';
import { codexAuthSettings, codexHome, type CodexAuthSettings } from './codex-auth.ts';
import { uuid7 } from '../../../common/identity/identity.ts';
import { resolveNativeCodex, type NativeCodexCommand } from './native-codex.ts';
import { prepareWslCodex, resolveWslCodex, type WslCodexContext, type WslCodexLaunch } from './wsl-codex.ts';
import { cliDecisionSchema, decodeCliDecision, type CliDecision } from './cli-decision.ts';

/** Native goals require persistence; user-input tools require an interactive session. Neither belongs in this stateless model transport. */
const disabledFeatures: string[] = ['shell_tool', 'unified_exec', 'multi_agent', 'multi_agent_v2', 'apps', 'plugins', 'hooks', 'code_mode_host', 'browser_use', 'browser_use_external', 'in_app_browser', 'computer_use', 'image_generation', 'skill_search', 'skill_mcp_dependency_install', 'view_image', 'sleep_tool', 'tool_suggest', 'goals', 'default_mode_request_user_input'];

/** Read catalog metadata only, excluding model instructions, credentials and local tools. */
export function codexCatalog(value: unknown): Model[] {
  const entries = z.object({ models: z.array(z.object({ slug: z.string(), display_name: z.string().optional(), context_window: z.number().optional(), max_output_tokens: z.number().optional(), supported_reasoning_levels: z.array(z.union([z.string(), z.object({ effort: z.string() })])).optional(), default_reasoning_level: z.string().optional() }).passthrough()) }).parse(value).models;
  return entries.filter((entry): boolean => typeof entry.context_window === 'number' && entry.context_window > 0).map((entry): Model => ({
    id: entry.slug, name: entry.display_name || entry.slug,
    reasoning_efforts: (entry.supported_reasoning_levels || []).map((effort): string => typeof effort === 'string' ? effort : effort.effort),
    default_reasoning_effort: entry.default_reasoning_level || null,
    context_window: entry.context_window!, max_output_tokens: entry.max_output_tokens || Math.floor(entry.context_window! / 5),
    tools: true, structured_output: true, input_modalities: ['text'],
  }));
}

/** One invocation asks for one logical decision; Codex can internally retry its own transport. */
export class CodexCliAdapter implements ProviderAdapter {
  async infer(request: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult> {
    const directory: string = await mkdtemp(path.join(os.tmpdir(), 'provider-router-'));
    try {
      const system: string = path.join(directory, 'system.md');
      const schema: string = path.join(directory, 'schema.json');
      const resultFile: string = path.join(directory, 'result.json');
      const catalogFile: string = path.join(directory, 'models.json');
      const wsl: WslCodexContext | null = connection.environment === 'wsl' ? await resolveWslCodex(connection.wsl_distribution!, { executable: connection.executable, codexHome: connection.codex_home }) : null;
      const sourceHome: string = wsl?.catalogDirectory || codexHome(connection.codex_home);
      const authentication: CodexAuthSettings = await codexAuthSettings(sourceHome);
      const invocationHome: string = wsl ? sourceHome : await isolatedAuthenticationHome(directory, sourceHome, authentication);
      const cache = z.object({ models: z.array(z.record(z.string(), z.unknown())) }).parse(JSON.parse(await readFile(path.join(sourceHome, 'models_cache.json'), 'utf8')));
      const cached: Record<string, unknown> | undefined = cache.models.find((model): boolean => model.slug === request.model);
      if (!cached) throw new RouterError('CODEX_MODEL_UNAVAILABLE', 'The selected model is absent from the installed Codex catalog. Refresh the catalog in Codex first.', 400);
      // apply_patch is exposed from model metadata independently of shell_tool.
      // Supply a private catalog without native tool declarations or model instructions.
      await writeFile(catalogFile, JSON.stringify({ models: [{ ...cached, base_instructions: 'Follow the provided system instructions.', apply_patch_tool_type: null, experimental_supported_tools: [], model_messages: null, include_skills_usage_instructions: false, include_plugin_usage_instructions: false, include_apps_usage_instructions: false, supports_search_tool: false, input_modalities: ['text'], tool_mode: 'direct' }] }), { mode: 0o600 });
      const instructions: string = request.messages.filter((message): boolean => message.role === 'system' || message.role === 'developer').map((message): string => typeof message.content === 'string' ? message.content : JSON.stringify(message.content)).join('\n\n');
      await writeFile(system, `${instructions}\n\nThe input contains the authoritative conversation and the caller's available functions. Continue that conversation for one inference decision. The available_functions entries are remote functions supplied and executed by the caller, independently of Codex's native capabilities. Request these functions through the decision format when the conversation calls for them. Native tool availability does not limit the caller's functions. The caller supplies their results in a subsequent request.`, { mode: 0o600 });
      await writeFile(schema, JSON.stringify(cliDecisionSchema(request)), { mode: 0o600 });
      const args: string[] = ['exec', '--ephemeral', '--ignore-user-config', '--ignore-rules', '--skip-git-repo-check', '--sandbox', 'read-only', '--json', '--model', request.model, '--output-schema', schema, '-o', resultFile, '-C', directory,
        '-c', `model_instructions_file=${JSON.stringify(system)}`, '-c', `model_catalog_json=${JSON.stringify(catalogFile)}`, '-c', 'approval_policy="never"', '-c', 'web_search="disabled"', '-c', 'project_doc_max_bytes=0', '--enable', 'skip_host_skill_discovery',
        ...disabledFeatures.flatMap((feature: string): string[] => ['--disable', feature]),
        '-c', `cli_auth_credentials_store=${JSON.stringify(authentication.store)}`, '-c', `features.secret_auth_storage=${authentication.secret_auth_storage}`,
      ];
      if (request.reasoning_effort) args.push('-c', `model_reasoning_effort=${JSON.stringify(request.reasoning_effort)}`);
      args.push('-');
      const input: string = JSON.stringify({ conversation: request.messages.filter((message): boolean => message.role !== 'system' && message.role !== 'developer'), available_functions: request.tools.map((tool) => tool.function), answer_schema: request.response_format?.json_schema.schema || null });
      const native: NativeCodexCommand | null = wsl ? null : await resolveNativeCodex(connection.executable || 'codex');
      const launch: CodexLaunch = wsl ? await prepareWslCodex(wsl, args, directory) : { executable: native!.executable, args: [...native!.prefix, ...args], environment: native!.environment };
      const usage: { input_tokens: number; output_tokens: number } = await executeCodex(launch, input, invocationHome, signal);
      const action: CliDecision = decodeCliDecision(JSON.parse(await readFile(resultFile, 'utf8')), request);
      const calls: ToolCall[] = action.tool_calls.map((call): ToolCall => ({ id: uuid7(), type: 'function', function: { name: call.name, arguments: call.arguments } }));
      return { request_id: request.request_id, content: action.content, tool_calls: calls, usage, provider: 'codex', method: 'codex_cli', model: request.model, ...(request.reasoning_effort ? { reasoning_effort: request.reasoning_effort } : {}) };
    } finally { await rm(directory, { recursive: true, force: true }); }
  }

  async models(connection: Connection, _signal: AbortSignal): Promise<Model[]> {
    try {
      const wsl: WslCodexContext | null = connection.environment === 'wsl' ? await resolveWslCodex(connection.wsl_distribution!, { executable: connection.executable, codexHome: connection.codex_home }) : null;
      return codexCatalog(JSON.parse(await readFile(path.join(wsl?.catalogDirectory || codexHome(connection.codex_home), 'models_cache.json'), 'utf8')));
    }
    catch { throw new RouterError('CODEX_CATALOG_UNAVAILABLE', 'Codex model metadata is unavailable. Start your installed Codex once to refresh its model catalog.', 503); }
  }
}

/** File logins share the real credential and commit lock without copying tokens.
 * Native keyring identities depend on the canonical home and retain that home.
 */
async function isolatedAuthenticationHome(directory: string, original: string, settings: CodexAuthSettings): Promise<string> {
  if (settings.store !== 'file') return original;
  const home: string = path.join(directory, 'codex-home');
  await mkdir(home, { mode: 0o700 });
  const lock: string = path.join(original, 'auth.json.lock');
  const descriptor = await open(lock, 'a+', 0o600); await descriptor.close();
  try {
    // Codex follows this symlink before atomic replacement; rotated credentials
    // are committed to their original destination under the original stable lock.
    await symlink(path.join(original, 'auth.json'), path.join(home, 'auth.json'), 'file');
    await symlink(lock, path.join(home, 'auth.json.lock'), 'file');
    return home;
  } catch (error: unknown) {
    // Windows symlinks can require Developer Mode. Do not copy credentials or
    // fabricate a second rotating store to hide optional user instructions.
    if (process.platform === 'win32' && error instanceof Error && 'code' in error && error.code === 'EPERM') return original;
    throw new RouterError('CODEX_ISOLATION_UNAVAILABLE', 'The existing Codex login could not be linked into an isolated session.', 503);
  }
}

/** Own the process tree until exit so cancellation cannot outlive temporary credential-free inputs. */
interface CodexLaunch { executable: string; args: string[]; environment?: Record<string, string>; cancel?: WslCodexLaunch['cancel'] }
async function executeCodex(launch: CodexLaunch, input: string, home: string, signal: AbortSignal): Promise<{ input_tokens: number; output_tokens: number }> {
  signal.throwIfAborted();
  return new Promise((resolve, reject): void => {
    const child: ChildProcessWithoutNullStreams = spawn(launch.executable, launch.args, { stdio: 'pipe', detached: process.platform !== 'win32', windowsHide: true, env: { ...cleanEnvironment(home), ...launch.environment } });
    let buffer: string = '';
    let bytes: number = 0;
    let completed: boolean = false;
    let failed: boolean = false;
    let usage: { input_tokens: number; output_tokens: number } = { input_tokens: 0, output_tokens: 0 };
    let killTimer: ReturnType<typeof setTimeout> | undefined;
    let stopping: Promise<void> | undefined;
    const stop: () => void = (): void => {
      stopping ??= (async (): Promise<void> => {
        // wsl.exe termination alone does not terminate its Linux descendants.
        // Stop the request-owned Linux group before killing the Windows launcher.
        if (launch.cancel) await launch.cancel();
        if (!child.pid) return;
        if (process.platform === 'win32') {
          await new Promise<void>((done): void => { const killer = spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' }); killer.once('error', (): void => { child.kill(); done(); }); killer.once('close', (): void => done()); });
        } else { try { process.kill(-child.pid, 'SIGTERM'); } catch { child.kill(); } killTimer = setTimeout((): void => { try { process.kill(-child.pid!, 'SIGKILL'); } catch { /* Already exited. */ } }, 1500); }
      })().catch((): void => { child.kill(); });
    };
    const deadline: ReturnType<typeof setTimeout> = setTimeout((): void => { failed = true; stop(); }, 600_000);
    signal.addEventListener('abort', stop, { once: true });
    child.on('error', (): void => { failed = true; });
    child.stdin.on('error', (): void => { /* An early process exit is handled below. */ });
    child.stderr.on('data', (): void => { /* Upstream stderr may contain private input. Never retain or expose it. */ });
    child.stdout.on('data', (chunk: Buffer): void => {
      bytes += chunk.length;
      if (bytes > 16_777_216) { failed = true; stop(); return; }
      buffer += chunk.toString();
      let newline: number;
      while ((newline = buffer.indexOf('\n')) >= 0) {
        const line: string = buffer.slice(0, newline); buffer = buffer.slice(newline + 1);
        try {
          const event: unknown = JSON.parse(line);
          if (typeof event !== 'object' || event === null) continue;
          const data: Record<string, unknown> = event as Record<string, unknown>;
          if (data.type === 'turn.completed') { completed = true; const u = z.object({ input_tokens: z.number().int().nonnegative(), output_tokens: z.number().int().nonnegative() }).safeParse(data.usage); if (u.success) usage = u.data; }
          if (data.type === 'turn.failed') { failed = true; stop(); }
          if ((data.type === 'item.started' || data.type === 'item.completed') && typeof data.item === 'object' && data.item !== null) {
            const item: Record<string, unknown> = data.item as Record<string, unknown>;
            if (['command_execution', 'file_change', 'mcp_tool_call', 'web_search', 'tool_call'].includes(String(item.type))) { failed = true; stop(); }
          }
        } catch { failed = true; stop(); }
      }
    });
    child.once('close', (code: number | null): void => {
      clearTimeout(deadline); signal.removeEventListener('abort', stop);
      void (async (): Promise<void> => {
        await stopping; clearTimeout(killTimer);
        if (signal.aborted) { reject(new RouterError('CANCELLED', 'The inference request was cancelled.', 499)); return; }
        if (code !== 0 || failed || !completed) { reject(new RouterError('CODEX_EXEC_FAILED', 'Codex did not complete. Check the installed CLI version and existing login.', 502)); return; }
        resolve(usage);
      })();
    });
    child.stdin.end(input);
  });
}

function cleanEnvironment(home: string): NodeJS.ProcessEnv {
  const output: NodeJS.ProcessEnv = { CODEX_HOME: home };
  const allowed: Set<string> = new Set(['PATH', 'PATHEXT', 'HOME', 'USERPROFILE', 'HOMEDRIVE', 'HOMEPATH', 'SYSTEMROOT', 'WINDIR', 'APPDATA', 'LOCALAPPDATA', 'PROGRAMFILES', 'PROGRAMFILES(X86)', 'TEMP', 'TMP', 'TMPDIR', 'LANG', 'LC_ALL', 'XDG_RUNTIME_DIR', 'DBUS_SESSION_BUS_ADDRESS', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY', 'SSL_CERT_FILE', 'SSL_CERT_DIR', 'NODE_EXTRA_CA_CERTS', 'CODEX_CA_CERTIFICATE', 'CURL_CA_BUNDLE', 'REQUESTS_CA_BUNDLE', 'BUNDLE_SSL_CA_CERT']);
  for (const [key, value] of Object.entries(process.env)) { if (allowed.has(key.toUpperCase()) && value) output[key] = value; }
  return output;
}
