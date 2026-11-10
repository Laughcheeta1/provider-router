/** Decode untrusted settings commands before dispatch. */
import { z } from 'zod';
import { connectionSchema, uuidSchema } from '../../../common/schemas/contracts.ts';
import { networkSchema } from './state.ts';
export const commandSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('snapshot') }),
  z.object({ type: z.literal('save_connection'), connection: connectionSchema, api_key: z.string().max(1024).optional() }),
  z.object({ type: z.literal('remove_connection'), id: uuidSchema }),
  z.object({ type: z.literal('refresh_models'), id: uuidSchema }),
  z.object({ type: z.literal('create_client'), name: z.string().min(1).max(100) }),
  z.object({ type: z.literal('revoke_client'), id: uuidSchema }),
  z.object({ type: z.literal('save_network'), network: networkSchema }),
  z.object({ type: z.literal('check_updates') }),
]);
