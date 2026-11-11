/** Local-only network policy shared by settings and API admission. */
import { RouterError } from '../../common/schemas/contracts.ts';
import type { NetworkSettings } from './schemas/state.ts';
/** Restrict the API listener to this computer. */
export function validateNetwork(network: NetworkSettings): void {
  if (!['127.0.0.1', '::1', 'localhost'].includes(network.host)) throw new RouterError('LOCAL_ONLY', 'Provider Router only supports loopback access on this computer.');
}
