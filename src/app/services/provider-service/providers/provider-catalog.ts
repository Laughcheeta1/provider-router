/** Decode the published provider catalogs using their native Rust field contracts. */
import { z } from 'zod';
import { hiddenCodexModel, RouterError, providerStateObjectSchema, type Model } from '../../../common/schemas/contracts.ts';
import { parseNativeJson, type NativeJsonShape } from '../../../common/schemas/native-json.ts';
import { nativeOption, nativeStruct } from '../../../common/schemas/native-struct.ts';
import { tokenCount, tokenCountSchema, type TokenCount } from '../../../common/schemas/token-count.ts';

const codexFields = {
  slug: true, display_name: true, context_window: true, max_output_tokens: true, default_reasoning_level: true,
  supported_reasoning_levels: true, input_modalities: true,
} satisfies NativeJsonShape;
const codexModel = nativeStruct(z.object({
  slug: z.string(), display_name: nativeOption(z.string()),
  context_window: nativeOption(tokenCountSchema), max_output_tokens: nativeOption(tokenCountSchema),
  default_reasoning_level: nativeOption(z.string()),
  supported_reasoning_levels: z.array(z.unknown()).default([]),
  input_modalities: z.array(z.string()).default([]),
}), undefined, 5, ['context_window', 'max_output_tokens']);
const codexCatalog = nativeStruct(z.object({ models: z.array(codexModel) }));

const openRouterFields = {
  id: true, name: true, context_length: true, top_provider: { max_completion_tokens: true },
  supported_parameters: true, architecture: { input_modalities: true }, reasoning: true,
} satisfies NativeJsonShape;
const openRouterModel = nativeStruct(z.object({
  id: z.string(), name: z.string(), context_length: nativeOption(tokenCountSchema),
  top_provider: nativeOption(nativeStruct(z.object({ max_completion_tokens: nativeOption(tokenCountSchema) }), undefined, undefined, ['max_completion_tokens'])),
  supported_parameters: z.array(z.string()).default([]),
  architecture: nativeOption(nativeStruct(z.object({ input_modalities: z.array(z.string()).default([]) }), undefined, 0)),
  reasoning: nativeOption(providerStateObjectSchema),
}), undefined, undefined, ['context_length']);
const openRouterCatalog = nativeStruct(z.object({ data: z.array(openRouterModel) }));

export function decodeCodexCatalog(source: Uint8Array): Model[] {
  const { models } = decode(codexCatalog, source, { models: [codexFields] });
  const result: Model[] = [];
  for (const entry of models) {
    const context: TokenCount | undefined = entry.context_window;
    if (!entry.slug || hiddenCodexModel(entry.slug) || !context) continue;
    const efforts: string[] = entry.supported_reasoning_levels.flatMap((value: unknown): string[] => {
      if (typeof value === 'string') return [value];
      if (value && typeof value === 'object' && 'effort' in value && typeof value.effort === 'string') return [value.effort];
      return [];
    });
    result.push({
      id: entry.slug, name: entry.display_name ?? entry.slug,
      reasoning_efforts: efforts, default_reasoning_effort: defaultEffort(entry.default_reasoning_level, efforts),
      context_window: context, max_output_tokens: outputLimit(entry.max_output_tokens, context),
      tools: true, structured_output: true, input_modalities: modalities(entry.input_modalities),
    });
  }
  return result;
}

export function decodeOpenRouterCatalog(source: Uint8Array): Model[] {
  const { data } = decode(openRouterCatalog, source, { data: [openRouterFields] });
  const result: Model[] = [];
  for (const entry of data) {
    const context: TokenCount | undefined = entry.context_length;
    if (!context) continue;
    const supported: unknown = entry.reasoning?.supported_efforts;
    const efforts: string[] = supported === null ? ['max', 'xhigh', 'high', 'medium', 'low', 'minimal', 'none']
      : Array.isArray(supported) ? supported.filter((effort: unknown): effort is string => typeof effort === 'string') : [];
    const allowed: string[] = entry.reasoning?.mandatory === true ? efforts.filter((effort: string): boolean => effort !== 'none') : efforts;
    result.push({
      id: entry.id, name: entry.name,
      reasoning_efforts: allowed, default_reasoning_effort: defaultEffort(entry.reasoning?.default_effort, allowed),
      context_window: context, max_output_tokens: outputLimit(entry.top_provider?.max_completion_tokens, context),
      tools: entry.supported_parameters.includes('tools'), structured_output: entry.supported_parameters.includes('structured_outputs'),
      input_modalities: modalities(entry.architecture?.input_modalities),
    });
  }
  return result;
}

function decode<T>(schema: z.ZodType<T>, source: Uint8Array, shape: NativeJsonShape): T {
  try { return schema.parse(parseNativeJson(source, shape)); }
  catch { throw new RouterError('PROVIDER_RESPONSE_INVALID', 'The provider returned an invalid response.', 502); }
}

function defaultEffort(value: unknown, allowed: string[]): string | null {
  return typeof value === 'string' && allowed.includes(value) ? value : null;
}

function outputLimit(limit: TokenCount | undefined, context: TokenCount): TokenCount {
  return limit || tokenCount(BigInt(context) / 5n || 1n);
}

function modalities(input?: string[]): string[] {
  return input?.includes('image') ? ['text', 'image'] : ['text'];
}
