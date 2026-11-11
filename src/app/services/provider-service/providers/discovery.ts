/** Inspect supported local CLI installations without copying credentials or enabling API access. */
import { spawn, type ChildProcess } from 'node:child_process';
import type { Connection, DetectedProvider } from '../../../common/schemas/contracts.ts';
import { uuid7 } from '../../../common/identity/identity.ts';
import { resolveNativeCodex, type NativeCodexCommand } from './native-codex.ts';
import { resolveWslCodex, type WslCodexContext } from './wsl-codex.ts';

export interface DiscoveryPorts {
  native(): Promise<NativeCodexCommand>;
  wsl(distribution: string): Promise<WslCodexContext>;
  succeeds(executable: string, args: readonly string[], environment: Record<string, string>): Promise<boolean>;
}

/** Candidate identities remain stable while settings refresh; persisted identities are assigned only once. */
export class LocalProviderDiscovery {
  private readonly identities: Map<string, string> = new Map();
  constructor(private readonly ports: DiscoveryPorts) {}

  async discover(distributions: readonly string[]): Promise<DetectedProvider[]> {
    return Promise.all([this.inspect(), ...distributions.map((distribution: string): Promise<DetectedProvider> => this.inspect(distribution))]);
  }

  private async inspect(distribution?: string): Promise<DetectedProvider> {
    const key: string = distribution ?? 'native';
    if (!this.identities.has(key)) this.identities.set(key, uuid7());
    const connection: Connection = { id: this.identities.get(key)!, name: distribution ? `Codex · ${distribution}` : 'Codex', method: 'codex_http', exposed: false, models: [], environment: distribution ? 'wsl' : 'native', ...(distribution ? { wsl_distribution: distribution } : {}) };
    try {
      let executable: string;
      let prefix: string[];
      let environment: Record<string, string>;
      if (distribution) {
        const context: WslCodexContext = await this.ports.wsl(distribution);
        executable = 'wsl.exe';
        prefix = ['--distribution', distribution, '--exec', 'env', `CODEX_HOME=${context.codexHome}`, `PATH=${context.searchPath}`, context.executable];
        environment = {};
      } else {
        const command: NativeCodexCommand = await this.ports.native();
        executable = command.executable; prefix = command.prefix; environment = command.environment;
      }
      if (!await this.ports.succeeds(executable, [...prefix, '--version'], environment)) return { connection, status: 'not_installed', detail: 'Install Codex CLI to connect this provider.' };
      const authenticated: boolean = await this.ports.succeeds(executable, [...prefix, 'login', 'status'], environment);
      return { connection, status: authenticated ? 'ready' : 'login_required', detail: authenticated ? 'Existing Codex login detected.' : 'Run codex login in this environment, then refresh.' };
    } catch {
      return { connection, status: 'not_installed', detail: 'Codex CLI was not found in this environment.' };
    }
  }
}

/** CLI probes have no model calls and discard all output, including account details. */
export function createLocalProviderDiscovery(): LocalProviderDiscovery {
  return new LocalProviderDiscovery({ native: resolveNativeCodex, wsl: resolveWslCodex, succeeds: probeCommand });
}

async function probeCommand(executable: string, args: readonly string[], environment: Record<string, string>): Promise<boolean> {
  return new Promise<boolean>((resolve): void => {
    const child: ChildProcess = spawn(executable, [...args], { shell: false, windowsHide: true, env: { ...process.env, ...environment }, stdio: 'ignore', timeout: 5000 });
    child.once('error', (): void => resolve(false));
    child.once('close', (code: number | null): void => resolve(code === 0));
  });
}
