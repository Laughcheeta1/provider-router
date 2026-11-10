/** Bind the caller's actual answer schema into Codex's one-decision output contract. */
import { z } from 'zod';
import { RouterError, type InferenceRequest } from '../../../common/schemas/contracts.ts';
// RevExt: 1
export interface CliDecision { content: string; tool_calls: { name: string; arguments: string }[] }
// RevExt: 2
/** A structured answer is a JSON value under its real schema, never an unconstrained JSON string. */
export function cliDecisionSchema(request: InferenceRequest): Record<string, unknown> {
  const choice: InferenceRequest['tool_choice'] = request.tool_choice;
  const permitted: string[] = request.tools.filter((tool): boolean => choice !== 'none' && (typeof choice !== 'object' || tool.function.name === choice.function.name)).map((tool): string => tool.function.name);
  const required: boolean = choice === 'required' || typeof choice === 'object';
  const alternatives: Record<string, unknown>[] = [];
  if (!required) alternatives.push({ type: 'object', additionalProperties: false, properties: request.response_format
    ? { kind: { type: 'string', enum: ['answer'] }, answer: { $ref: '#/$defs/response' } }
    : { kind: { type: 'string', enum: ['answer'] }, content: { type: 'string' } }, required: request.response_format ? ['kind', 'answer'] : ['kind', 'content'] });
  if (permitted.length) alternatives.push({ type: 'object', additionalProperties: false, properties: {
    kind: { type: 'string', enum: ['tools'] },
    tool_calls: { type: 'array', minItems: 1, ...(request.parallel_tool_calls === false ? { maxItems: 1 } : {}), items: { type: 'object', additionalProperties: false, properties: { name: { type: 'string', enum: permitted }, arguments: { type: 'string', description: 'A serialized JSON object matching the selected function parameters.' } }, required: ['name', 'arguments'] } },
  }, required: ['kind', 'tool_calls'] });
  if (!alternatives.length) throw new RouterError('INVALID_TOOL_CHOICE', 'The selected tool choice has no available function.');
  return { type: 'object', additionalProperties: false, properties: { decision: { anyOf: alternatives } }, required: ['decision'], ...(request.response_format ? { $defs: { response: embedSchema(request.response_format.json_schema.schema) } } : {}) };
}  // RevExt: 6
// RevExt: 3
/** Normalize a typed decision to API text/tool requests after decoding, not through prompt instructions. */
export function decodeCliDecision(value: unknown, request: InferenceRequest): CliDecision {
  const envelope: { decision: Record<string, unknown> } = z.object({ decision: z.record(z.string(), z.unknown()) }).strict().parse(value);
  if (envelope.decision.kind === 'tools') {
    const tools: { kind: 'tools'; tool_calls: { name: string; arguments: string }[] } = z.object({ kind: z.literal('tools'), tool_calls: z.array(z.object({ name: z.string(), arguments: z.string() }).strict()).min(1) }).strict().parse(envelope.decision);
    return { content: '', tool_calls: tools.tool_calls };
  }
  if (request.response_format) {
    const answer: { kind: 'answer'; answer?: unknown } = z.object({ kind: z.literal('answer'), answer: z.unknown() }).strict().parse(envelope.decision);
    if (!Object.hasOwn(answer, 'answer')) throw new RouterError('INVALID_COMPLETION', 'Codex omitted the requested structured answer.', 502);
    const content: string | undefined = JSON.stringify(answer.answer);
    if (content === undefined) throw new RouterError('INVALID_COMPLETION', 'Codex returned an unrepresentable structured answer.', 502);
    return { content, tool_calls: [] };
  }
  const answer: { kind: 'answer'; content: string } = z.object({ kind: z.literal('answer'), content: z.string() }).strict().parse(envelope.decision);
  return { content: answer.content, tool_calls: [] };
}  // RevExt: 7
// RevExt: 4
/** Rebase local references without rewriting literal const/enum/example application data. */
function embedSchema(value: unknown): unknown {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return value;
  const schema: Record<string, unknown> = value as Record<string, unknown>;
  const embedded: Record<string, unknown> = {};
  const maps: Set<string> = new Set(['$defs', 'definitions', 'properties', 'patternProperties', 'dependentSchemas']);
  const arrays: Set<string> = new Set(['allOf', 'anyOf', 'oneOf', 'prefixItems']);
  const singles: Set<string> = new Set(['items', 'contains', 'additionalProperties', 'additionalItems', 'unevaluatedProperties', 'unevaluatedItems', 'propertyNames', 'not', 'if', 'then', 'else']);
  for (const [key, field] of Object.entries(schema)) {
    if (key === '$id') continue;
    if ((key === '$ref' || key === '$dynamicRef') && typeof field === 'string') {
      embedded[key] = rebaseReference(field);
    } else if (key === 'discriminator' && typeof field === 'object' && field !== null && !Array.isArray(field)) {
      const discriminator: Record<string, unknown> = field as Record<string, unknown>;
      const mapping: unknown = discriminator.mapping;
      embedded[key] = { ...discriminator, ...(typeof mapping === 'object' && mapping !== null && !Array.isArray(mapping)
        ? { mapping: Object.fromEntries(Object.entries(mapping).map(([name, target]: [string, unknown]): [string, unknown] => [name, typeof target === 'string' ? rebaseReference(target) : target])) } : {}) };
    } else if ((key === '$anchor' || key === '$dynamicAnchor') && typeof field === 'string') embedded[key] = `provider_router_response_${field}`;
    else if (maps.has(key) && typeof field === 'object' && field !== null && !Array.isArray(field)) embedded[key] = Object.fromEntries(Object.entries(field).map(([name, nested]: [string, unknown]): [string, unknown] => [name, embedSchema(nested)]));
    else if ((arrays.has(key) || key === 'items') && Array.isArray(field)) embedded[key] = field.map(embedSchema);
    else if (singles.has(key)) embedded[key] = embedSchema(field);
    else embedded[key] = field;
  }
  return embedded;
}  // RevExt: 8
// RevExt: 5
/** Discriminator mappings are schema references too, while const/enum values remain literal data. */
function rebaseReference(value: string): string {
  if (!value.startsWith('#')) throw new RouterError('UNSUPPORTED_SCHEMA', 'Codex CLI structured output requires local JSON Schema references.');
  return value === '#' ? '#/$defs/response' : value.startsWith('#/') ? `#/$defs/response${value.slice(1)}` : `#provider_router_response_${value.slice(1)}`;
}  // RevExt: 9
