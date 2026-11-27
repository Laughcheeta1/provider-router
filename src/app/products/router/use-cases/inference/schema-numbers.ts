/** Match numeric keywords in the native jsonschema 0.30 validator. */
import type { Ajv, KeywordDefinition } from 'ajv';
import type { DataValidationCxt } from 'ajv/dist/types/index.js';
import { nativeSchemaNumber } from '../../../../common/schemas/native-json.ts';

/** Install the native numeric rules before compiling either schemas or meta-schemas. */
export function useNativeNumbers(ajv: Ajv, draft4: boolean, usesDraft4: (schema: object) => boolean | undefined): Ajv {
  const bounds: [string, KeywordDefinition][] = [
    ['minimum', bound('minimum', true, draft4, false)],
    ['maximum', bound('maximum', false, draft4, false)],
  ];
  if (!draft4) {
    bounds.push(
      ['exclusiveMinimum', bound('exclusiveMinimum', true, false, true)],
      ['exclusiveMaximum', bound('exclusiveMaximum', false, false, true)],
    );
  }

  for (const [keyword, definition] of bounds) ajv.removeKeyword(keyword).addKeyword(definition);
  return ajv.removeKeyword('const').addKeyword({
    keyword: 'const', errors: false,
    compile: (expected: unknown, schema: object): InstanceCheck => {
      if (usesDraft4(schema) ?? draft4) return (): boolean => true;
      // Native's scalar const uses epsilon on f64. Containers use exact numeric equality.
      if (typeof expected === 'number') {
        return (value: unknown): boolean => typeof value === 'number' && Math.abs(expected - value) < Number.EPSILON;
      }
      const key: string = equalityKey(expected, schema, 'const');
      return (value: unknown, context?: DataValidationCxt): boolean => key === instanceKey(value, context);
    },
  }).removeKeyword('type').addKeyword({
    keyword: 'type', schemaType: ['string', 'array'], errors: false,
    compile: (types: string | string[], schema: object): InstanceCheck => {
      if (!(usesDraft4(schema) ?? draft4)) return (): boolean => true;
      const accepted: string[] = typeof types === 'string' ? [types] : types;
      if (!accepted.includes('integer') || accepted.includes('number')) return (): boolean => true;
      // Ajv checks the shape. Draft 4 also requires a native integer token.
      return (value: unknown, context?: DataValidationCxt): boolean =>
        typeof value !== 'number' || typeof instanceNumber(value, context) === 'bigint';
    },
  }).removeKeyword('multipleOf').addKeyword({
    keyword: 'multipleOf', type: 'number', schemaType: 'number', errors: false, validate: isMultiple,
  }).removeKeyword('enum').addKeyword({
    keyword: 'enum', schemaType: 'array', errors: false,
    compile: (values: unknown[]): InstanceCheck => {
      const keys: Set<string> = new Set(values.map((value: unknown, index: number): string => equalityKey(value, values, index)));
      return (value: unknown, context?: DataValidationCxt): boolean => keys.has(instanceKey(value, context));
    },
  }).removeKeyword('uniqueItems').addKeyword({
    keyword: 'uniqueItems', type: 'array', schemaType: 'boolean', errors: false,
    validate: (unique: boolean, values: unknown[]): boolean => {
      if (!unique) return true;
      const seen: Set<string> = new Set();
      // Native switches to hashing f64 bit patterns above 15 items, which distinguishes signed zero.
      const hashZeroSign: boolean = values.length > 15;
      for (const [index, value] of values.entries()) {
        const key: string = equalityKey(value, values, index, hashZeroSign);
        if (seen.has(key)) return false;
        seen.add(key);
      }
      return true;
    },
  });
}

type InstanceCheck = (value: unknown, context?: DataValidationCxt) => boolean;

function instanceNumber(value: number, context?: DataValidationCxt): bigint | number {
  return nativeSchemaNumber(value, context?.parentData, context?.parentDataProperty);
}

function instanceKey(value: unknown, context?: DataValidationCxt): string {
  return equalityKey(value, context?.parentData, context?.parentDataProperty);
}

function bound(keyword: string, lower: boolean, draft4: boolean, exclusive: boolean): KeywordDefinition {
  return {
    keyword, type: 'number', schemaType: 'number', errors: false,
    compile: (schemaValue: number, schema: Record<string, unknown>) => {
      const limit: number | bigint = nativeSchemaNumber(schemaValue, schema, keyword);
      const strict: boolean = exclusive || (draft4 && schema[lower ? 'exclusiveMinimum' : 'exclusiveMaximum'] === true);
      return (value: number, context?: DataValidationCxt): boolean => {
        const actual: number | bigint = instanceNumber(value, context);
        if (lower) return strict ? actual > limit : actual >= limit;
        return strict ? actual < limit : actual <= limit;
      };
    },
  };
}

function equalityKey(value: unknown, parent?: object, key?: string | number, hashZeroSign: boolean = false): string {
  if (typeof value === 'number') {
    const exact: number | bigint = nativeSchemaNumber(value, parent, key);
    if (hashZeroSign && Object.is(exact, -0)) return '-0';
    // The native comparator equates integers and integral floats only at the same exact value.
    return typeof exact === 'bigint' || !Number.isInteger(exact) ? String(exact) : String(BigInt(exact));
  }
  if (Array.isArray(value)) {
    return `[${value.map((item: unknown, index: number): string => equalityKey(item, value, index, hashZeroSign)).join(',')}]`;
  }
  if (value !== null && typeof value === 'object') {
    const object: Record<string, unknown> = value as Record<string, unknown>;
    return `{${Object.keys(object).sort().map((name: string): string => `${JSON.stringify(name)}:${equalityKey(object[name], object, name, hashZeroSign)}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

function isMultiple(divisor: number, value: number): boolean {
  // Native converts both operands to f64, and uses a different remainder for fractional divisors.
  if (Number.isInteger(divisor)) return Number.isInteger(value) && value % divisor === 0;
  const quotient: number = value / divisor;
  if (Number.isFinite(quotient)) return quotient % 1 < Number.EPSILON;

  // jsonschema falls back to fraction::BigFraction when the f64 quotient overflows.
  const [numerator, scale]: [bigint, bigint] = decimalFraction(value);
  const [divisorNumerator, divisorScale]: [bigint, bigint] = decimalFraction(divisor);
  return divisorNumerator === 0n || (numerator * divisorScale) % (divisorNumerator * scale) === 0n;
}

function decimalFraction(value: number): [bigint, bigint] {
  let precision: number = 0;
  let scaled: number = value;
  let scale: number = 1;
  while (Math.abs(Math.floor(scaled) - scaled) >= Number.EPSILON) {
    scale = rustPowerOfTen(++precision);
    scaled = value * scale;
  }
  return [BigInt(Math.trunc(scaled)), BigInt(scale)];
}

function rustPowerOfTen(exponent: number): number {
  // Rust's f64::powi multiplies by squares. JS exponentiation rounds some of these scales differently.
  let power: number = 1;
  for (let factor: number = 10; exponent > 0; exponent = Math.floor(exponent / 2)) {
    if (exponent % 2 === 1) power *= factor;
    factor *= factor;
  }
  return power;
}
