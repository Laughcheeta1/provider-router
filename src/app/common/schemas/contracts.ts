/** Versioned, provider-neutral HTTP contracts; no application account is required. */
import { z } from 'zod';
import { nativeOption, nativeStruct, nativeUnsignedFields } from './native-struct.ts';
import { tokenCountSchema } from './token-count.ts';

export const PROTOCOL_VERSION: 1 = 1;
export const DEFAULT_PORT: number = 47831;
export const uuidSchema = z.string().uuid().refine((value: string): boolean => value[14] === '7', 'UUIDv7 required');
export const jsonObjectSchema = z.record(z.string(), z.unknown());
const positiveTokenCount = tokenCountSchema.refine((count): boolean => count > 0);
export const modelSchema = nativeUnsignedFields(z.object({
  id: z.string().min(1), name: z.string().min(1), reasoning_efforts: z.array(z.string()),
  default_reasoning_effort: z.string().nullable(), context_window: positiveTokenCount,
  max_output_tokens: positiveTokenCount, tools: z.boolean(), structured_output: z.boolean(),
  input_modalities: z.array(z.string()),
}).superRefine((model, context): void => {
  if (model.default_reasoning_effort !== null && !model.reasoning_efforts.includes(model.default_reasoning_effort)) context.addIssue({ code: 'custom', message: 'The default thinking effort must be allowed.' });
}), ['context_window', 'max_output_tokens']);
export type Model = z.infer<typeof modelSchema>;
export const methodSchema = z.enum(['codex_cli', 'codex_http', 'openrouter_http']);
export type Method = z.infer<typeof methodSchema>;
export const connectionSchema = z.object({
  id: uuidSchema, name: z.string().trim().min(1).max(100), method: methodSchema,
  exposed: z.boolean(), models: z.array(modelSchema),
  codex_home: z.string().optional(), executable: z.string().optional(),
  environment: z.enum(['native', 'wsl']).optional(), wsl_distribution: z.string().min(1).max(128).optional(),
}).superRefine((connection, context): void => {
  if (connection.environment === 'wsl' && (connection.method === 'openrouter_http' || !connection.wsl_distribution)) {
    context.addIssue({ code: 'custom', message: 'A WSL connection requires Codex and a selected distribution.' });
  }
}).transform((connection) => ({ ...connection, method: connection.method === 'codex_cli' ? 'codex_http' as const : connection.method, models: connection.method === 'openrouter_http' ? connection.models : connection.models.filter((model): boolean => !hiddenCodexModel(model.id)) }));
export type Connection = z.infer<typeof connectionSchema>;
/** Local provider availability contains no credential material or inferred model access. */
export interface DetectedProvider { connection: Connection; status: 'ready' | 'login_required' | 'not_installed'; detail: string }
export const nativeUuidSchema = z.string().transform((input: string, context): string => {
  const wrapped: string = input.slice(0, 9).toLowerCase() === 'urn:uuid:' ? input.slice(9) : input.startsWith('{') && input.endsWith('}') ? input.slice(1, -1) : input;
  const match: RegExpMatchArray | null = wrapped.match(/^([0-9a-f]{8})-?([0-9a-f]{4})-?([0-9a-f]{4})-?([0-9a-f]{4})-?([0-9a-f]{12})$/i);
  if (!match || (wrapped.length !== 36 && (wrapped.length !== 32 || wrapped !== input))) {
    context.addIssue({ code: 'custom', message: 'Expected a UUID.' });
    return z.NEVER;
  }
  return match.slice(1).join('-').toLowerCase();
});
export const inferenceIdSchema = nativeUuidSchema.refine((value: string): boolean => value[14] === '7', 'UUIDv7 required');

export function hiddenCodexModel(id: string): boolean {
  return id === 'gpt-reserve' || id === 'codex-auto-review';
}

const toolInvocationSchema = nativeStruct(z.strictObject({ name: z.string(), arguments: z.string() }));
export const completionToolCallSchema = z.strictObject({ id: z.string(), type: z.literal('function'), function: toolInvocationSchema });
export type ToolCall = z.infer<typeof completionToolCallSchema>;
export const filePartSchema = z.strictObject({
  type: z.literal('file'),
  file: nativeStruct(z.strictObject({ filename: z.string(), file_data: z.string() })),
});
const partSchema = z.discriminatedUnion('type', [
  z.strictObject({ type: z.literal('text'), text: z.string() }),
  filePartSchema,
  z.strictObject({ type: z.literal('image_url'), image_url: nativeStruct(z.strictObject({ url: z.string() })) }),
]);
const jsonSchema: z.ZodJSONSchema = z.json();
// Validate opaque state without cloning its values and losing their JSON number tokens.
export const providerStateValueSchema = z.custom<z.infer<typeof jsonSchema>>((value: unknown): boolean => jsonSchema.safeParse(value).success);
export const providerStateObjectSchema = z.custom<Record<string, unknown>>((value: unknown): boolean => value !== null && typeof value === 'object' && !Array.isArray(value));
/** Provider-owned assistant state scoped to the connection and model that produced it. */
const continuationSchema = nativeStruct(z.strictObject({
  connection_id: nativeUuidSchema,
  model: z.string(),
  state: z.discriminatedUnion('format', [
    z.strictObject({ format: z.literal('codex_responses'), items: z.array(providerStateValueSchema) }),
    z.strictObject({ format: z.literal('openrouter_chat'), message: providerStateObjectSchema, upstream: z.string(), resolved_model: z.string() }),
  ]),
}));
export type Continuation = z.infer<typeof continuationSchema>;
export const roleSchema = z.preprocess((value: unknown): unknown => {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return value;
  const fields: [string, unknown][] = Object.entries(value);
  return fields.length === 1 && fields[0]?.[1] === null ? fields[0][0] : value;
}, z.enum(['system', 'developer', 'user', 'assistant', 'tool']));
export const messageSchema = nativeStruct(z.strictObject({
  role: roleSchema,
  content: z.union([z.string(), z.array(partSchema).transform((parts): Record<string, unknown>[] => parts), z.null()]),
  tool_calls: nativeOption(z.array(completionToolCallSchema)),
  tool_call_id: nativeOption(z.string()),
  name: nativeOption(z.string()),
  continuation: nativeOption(continuationSchema),
}));
export type Message = z.infer<typeof messageSchema>;
export const functionSchema = z.strictObject({
  name: z.string(), description: nativeOption(z.string()), parameters: providerStateObjectSchema, strict: nativeOption(z.boolean()),
});
export const toolSchema = z.strictObject({ type: z.literal('function'), function: nativeStruct(functionSchema) });
export type Tool = z.infer<typeof toolSchema>;
export const structuredSchema = z.strictObject({ name: z.string(), schema: providerStateObjectSchema, strict: nativeOption(z.boolean()) });
export const formatSchema = z.strictObject({ type: z.literal('json_schema'), json_schema: nativeStruct(structuredSchema) });
const namedChoiceSchema = z.object({ type: z.literal('function'), function: z.object({ name: z.string().min(1) }) });
export const toolChoiceSchema = nativeOption(z.union([
  z.enum(['auto', 'none', 'required']),
  z.custom<z.infer<typeof namedChoiceSchema>>((value: unknown): boolean => namedChoiceSchema.safeParse(value).success),
]));
const inferenceShape = z.strictObject({
  request_id: inferenceIdSchema, connection_id: inferenceIdSchema, model: z.string(),
  messages: z.array(messageSchema), tools: z.array(toolSchema).default([]),
  reasoning_effort: nativeOption(z.string()), response_format: nativeOption(formatSchema),
  max_output_tokens: nativeOption(tokenCountSchema), tool_choice: z.unknown().optional(),
  parallel_tool_calls: nativeOption(z.boolean()), session_id: nativeOption(nativeUuidSchema),
});
export const inferenceSchema = nativeStruct(inferenceShape, undefined, undefined, ['max_output_tokens']);
export type InferenceBody = z.infer<typeof inferenceSchema>;
export type InferenceRequest = Omit<InferenceBody, 'tool_choice'> & { tool_choice?: z.infer<typeof toolChoiceSchema> };
export const resultSchema = z.object({
  request_id: inferenceIdSchema, content: z.string(), tool_calls: z.array(completionToolCallSchema),
  usage: z.object({
    input_tokens: tokenCountSchema, output_tokens: tokenCountSchema,
    cached_input_tokens: nativeOption(tokenCountSchema), cache_write_tokens: nativeOption(tokenCountSchema),
  }),
  provider: z.enum(['codex', 'openrouter']), method: methodSchema, model: z.string(), reasoning_effort: nativeOption(z.string()),
  continuation: nativeOption(continuationSchema),
});
export type InferenceResult = z.infer<typeof resultSchema>;
export interface ProviderDescriptor { id: string; name: string; provider: 'codex' | 'openrouter'; method: Method; models: Model[] }
export interface ProviderAdapter {
  infer(request: InferenceRequest, connection: Connection, signal: AbortSignal): Promise<InferenceResult>;
  models(connection: Connection, signal: AbortSignal): Promise<Model[]>;
}
/** A stable safe failure, with upstream text deliberately excluded. */
export class RouterError extends Error {
  constructor(public readonly code: string, message: string, public readonly status: number = 400) { super(message); }
}
