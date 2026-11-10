/** Network exposure policy shared by settings and API admission. */
import { RouterError } from '../../common/schemas/contracts.ts';
import type { NetworkSettings } from './schemas/state.ts';
/** Only explicit TLS or an explicit loopback TLS-terminating proxy can expose remote requests. */
export function validateNetwork(network: NetworkSettings): void {
  const loopback: boolean = ['127.0.0.1', '::1', 'localhost'].includes(network.host);
  if (!network.remote_enabled && (!loopback || network.trusted_proxy)) throw new RouterError('REMOTE_DISABLED', 'Enable remote access before changing the loopback listener.');
  if (network.remote_enabled && !network.trusted_proxy && (!network.tls_certificate || !network.tls_key)) throw new RouterError('TLS_REQUIRED', 'Remote listeners require a TLS certificate and private key.');
  if (network.trusted_proxy && (!loopback || !network.public_endpoint?.startsWith('https://'))) throw new RouterError('PROXY_INVALID', 'A trusted reverse proxy requires a loopback listener and an HTTPS public endpoint.');
  if (Boolean(network.tls_certificate) !== Boolean(network.tls_key)) throw new RouterError('TLS_REQUIRED', 'Supply both TLS certificate and private key paths.');
}
