/** Validated persisted state and safe settings projections. */
import { z } from 'zod';
import { connectionSchema, uuidSchema, type Connection } from '../../../common/schemas/contracts.ts';
export const networkSchema = z.object({
  port: z.number().int().min(1).max(65535).default(47831),
  require_api_key: z.boolean().default(false),
  browser_origins: z.array(z.string().refine((value: string): boolean => {
    try {
      const origin: URL = new URL(value);
      return origin.origin === value && (origin.protocol === 'https:' || (origin.protocol === 'http:' && ['localhost', '127.0.0.1', '[::1]'].includes(origin.hostname)));
    } catch { return false; }
  }, 'Use an exact HTTPS website origin, or an HTTP loopback development origin, without a path or trailing slash.')).max(50).optional(),
  host: z.string().refine((host: string): boolean => ['127.0.0.1', '::1', 'localhost'].includes(host), 'Use a loopback host: 127.0.0.1, ::1 or localhost.').default('127.0.0.1'),
});
export type NetworkSettings = z.infer<typeof networkSchema>;
export const clientSchema = z.object({ id: uuidSchema, name: z.string().min(1).max(100), token_hash: z.string(), created_at: z.string() });
export type Client = z.infer<typeof clientSchema>;
export const stateSchema = z.object({ version: z.literal(1), instance_id: uuidSchema, connections: z.array(connectionSchema), clients: z.array(clientSchema), secrets: z.record(z.string(), z.string()), network: networkSchema });
export type State = z.infer<typeof stateSchema>;
export interface PublicState { instance_id: string; connections: Connection[]; clients: Omit<Client, 'token_hash'>[]; network: NetworkSettings }
