/** Versioned, provider-neutral HTTP contracts; no application account is required. */
import { z } from 'zod';

export const PROTOCOL_VERSION: 1 = 1;
export const DEFAULT_PORT: number = 47831;
export const uuidSchema = z.string().uuid().refine((value: string): boolean => value[14] === '7', 'UUIDv7 required');
export const jsonObjectSchema = z.record(z.string(), z.unknown());
export const modelSchema = z.object({
  id: z.string().min(1), name: z.string().min(1), reasoning_efforts: z.array(z.string()),
  default_reasoning_effort: z.string().nullable(), context_window: z.number().int().positive(),
  max_output_tokens: z.number().int().positive(), tools: z.boolean(), structured_output: z.boolean(),
  input_modalities: z.array(z.string()),
}).superRefine((model, context): void => {
  if (model.default_reasoning_effort !== null && !model.reasoning_efforts.includes(model.default_reasoning_effort)) context.addIssue({ code: 'custom', message: 'The default thinking effort must be allowed.' });
});
export type Model = z.infer<typeof modelSchema>;
export const methodSchema = z.enum(['codex_cli', 'codex_http', 'openrouter_http']);
export type Method = z.infer<typeof methodSchema>;
export const connectionSchema = z.object({
  id: uuidSchema, name: z.string().trim().min(1).max(100), method: methodSchema,
  exposed: z.boolean(), models: z.array(modelSchema),
  codex_home: z.string().optional(), executable: z.string().optional(),
  environment: z.enum(['native', 'wsl']).optional(), wsl_distribution: z.string().min(1).max(128).optional(),
}).superRefine((connection, context): void => {
  if (connection.environment === 'wsl' && (connection.method !== 'codex_cli' || !connection.wsl_distribution)) {
    context.addIssue({ code: 'custom', message: 'A WSL connection requires Codex CLI and a selected distribution.' });
  }
});
export type Connection = z.infer<typeof connectionSchema>;
export const toolCallSchema = z.object({ id: z.string().min(1), type: z.literal('function'), function: z.object({ name: z.string().min(1), arguments: z.string() }).strict() }).strict();
export type ToolCall = z.infer<typeof toolCallSchema>;
export const messageSchema = z.object({
  role: z.enum(['system', 'developer', 'user', 'assistant', 'tool']),
  content: z.union([z.string(), z.array(jsonObjectSchema), z.null()]),
  tool_calls: z.array(toolCallSchema).optional(), tool_call_id: z.string().optional(), name: z.string().optional(),
}).strict();
export type Message = z.infer<typeof messageSchema>;
export const toolSchema = z.object({ type: z.literal('function'), function: z.object({ name: z.string().min(1), description: z.string().optional(), parameters: jsonObjectSchema, strict: z.boolean().optional() }).strict() }).strict();
export type Tool = z.infer<typeof toolSchema>;
export const inferenceSchema = z.object({
  request_id: uuidSchema, connection_id: uuidSchema, model: z.string().min(1), reasoning_effort: z.string().optional(),
  messages: z.array(messageSchema).min(1), tools: z.array(toolSchema).default([]),
  response_format: z.object({ type: z.literal('json_schema'), json_schema: z.object({ name: z.string(), schema: jsonObjectSchema, strict: z.boolean().optional() }) }).optional(),
  max_output_tokens: z.number().int().positive().optional(),
  tool_choice: z.union([z.enum(['auto', 'none', 'required']), z.object({ type: z.literal('function'), function: z.object({ name: z.string().min(1) }) })]).optional(),
  parallel_tool_calls: z.boolean().optional(),
}).strict();
export type InferenceRequest = z.infer<typeof inferenceSchema>;
export const resultSchema = z.object({
  request_id: uuidSchema, content: z.string(), tool_calls: z.array(toolCallSchema),
  usage: z.object({ input_tokens: z.number().int().nonnegative(), output_tokens: z.number().int().nonnegative() }),
  provider: z.enum(['codex', 'openrouter']), method: methodSchema, model: z.string(), reasoning_effort: z.string().optional(),
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
