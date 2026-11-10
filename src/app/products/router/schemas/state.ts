/** Validated persisted state and safe settings projections. */
import { z } from 'zod';
import { connectionSchema, uuidSchema, type Connection } from '../../../common/schemas/contracts.ts';
export const networkSchema = z.object({
  port: z.number().int().min(1).max(65535).default(47831),
  require_api_key: z.boolean().default(false),
  remote_enabled: z.boolean().default(false),
  host: z.string().default('127.0.0.1'),
  tls_certificate: z.string().optional(), tls_key: z.string().optional(),
  trusted_proxy: z.boolean().default(false), public_endpoint: z.string().url().optional(),
});
export type NetworkSettings = z.infer<typeof networkSchema>;
export const clientSchema = z.object({ id: uuidSchema, name: z.string().min(1).max(100), token_hash: z.string(), created_at: z.string() });
export type Client = z.infer<typeof clientSchema>;
export const stateSchema = z.object({ version: z.literal(1), instance_id: uuidSchema, connections: z.array(connectionSchema), clients: z.array(clientSchema), secrets: z.record(z.string(), z.string()), network: networkSchema });
export type State = z.infer<typeof stateSchema>;
export interface PublicState { instance_id: string; connections: Connection[]; clients: Omit<Client, 'token_hash'>[]; network: NetworkSettings }
