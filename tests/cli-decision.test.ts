/** Codex receives the actual answer contract, including local references and tool alternatives. */
import { Ajv } from 'ajv';
import { describe, expect, it } from 'vitest';
import { cliDecisionSchema, decodeCliDecision } from '../src/app/services/provider-service/providers/cli-decision.ts';
import { uuid7 } from '../src/app/common/identity/identity.ts';
import type { InferenceRequest } from '../src/app/common/schemas/contracts.ts';

function request(): InferenceRequest {
  return { request_id: uuid7(), connection_id: uuid7(), model: 'synthetic-model', messages: [{ role: 'user', content: 'Synthetic question' }], tools: [], response_format: { type: 'json_schema', json_schema: { name: 'business_answer', schema: {
    type: 'object', additionalProperties: false, properties: { summary: { type: 'string' }, facts: { type: 'array', items: { $ref: '#/$defs/Fact' } }, signature: { const: { $ref: '#literal-application-data' } } }, required: ['summary', 'facts', 'signature'],
    $defs: { Fact: { type: 'object', additionalProperties: false, properties: { name: { type: 'string' }, amount: { type: 'integer' }, source: { $ref: '#/$defs/Source' } }, required: ['name', 'amount', 'source'] }, Source: { type: 'object', additionalProperties: false, properties: { label: { type: 'string' }, present: { type: 'boolean' } }, required: ['label', 'present'] } },
  } } } };
}

describe('Codex decision schema', (): void => {
  it('enforces nested final answer fields and references at the provider boundary', (): void => {
    const input: InferenceRequest = request(); const validate = new Ajv({ strict: false }).compile(cliDecisionSchema(input));
    const answer: Record<string, unknown> = { summary: 'Synthetic result', facts: [{ name: 'account-a', amount: 7, source: { label: 'fixture', present: true } }], signature: { $ref: '#literal-application-data' } };
    const output: Record<string, unknown> = { decision: { kind: 'answer', answer } };
    expect(validate(output)).toBe(true); expect(decodeCliDecision(output, input)).toEqual({ content: JSON.stringify(answer), tool_calls: [] });
    expect(validate({ decision: { kind: 'answer', content: JSON.stringify(answer) } })).toBe(false);
    expect(validate({ decision: { kind: 'answer', answer: { ...answer, facts: [{ name: 'a', amount: 'wrong', source: { label: 'fixture', present: true } }] } } })).toBe(false);
  });

  it('permits tool requests without a simultaneous final answer and enforces forced single-function choice', (): void => {
    const input: InferenceRequest = { ...request(), tools: [{ type: 'function', function: { name: 'read_balance', parameters: { type: 'object' } } }], tool_choice: { type: 'function', function: { name: 'read_balance' } }, parallel_tool_calls: false };
    const validate = new Ajv({ strict: false }).compile(cliDecisionSchema(input));
    const call: Record<string, unknown> = { name: 'read_balance', arguments: '{"id":"a"}' };
    expect(validate({ decision: { kind: 'tools', tool_calls: [call] } })).toBe(true);
    expect(validate({ decision: { kind: 'tools', tool_calls: [call, call] } })).toBe(false);
    expect(validate({ decision: { kind: 'tools', tool_calls: [{ ...call, name: 'unavailable' }] } })).toBe(false);
    expect(validate({ decision: { kind: 'answer', answer: {} } })).toBe(false);
  });
});
