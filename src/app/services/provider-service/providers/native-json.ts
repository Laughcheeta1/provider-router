/** Decode the same JSON values as the native router's default serde_json parser. */
import { isDeepStrictEqual } from 'node:util';
import { rememberNumberSource } from '../../../common/schemas/json-wire.ts';
import { MAX_U64 } from '../../../common/schemas/token-count.ts';

type NativeValue = null | boolean | string | bigint | number | NativeValue[] | { [key: string]: NativeValue };
const MIN_I64: bigint = -9_223_372_036_854_775_808n;
const MAX_I32: number = 2_147_483_647;

export function replayArgumentsEqual(expected: string, actual: string): boolean {
  try {
    return isDeepStrictEqual(readNativeJson(expected, false), readNativeJson(actual, false));
  } catch {
    return false;
  }
}

/** Provider decoders consume ordinary JS values and can recover exact number tokens when needed. */
export function parseNativeProviderJson(source: string): unknown {
  return readNativeJson(source, true);
}

function readNativeJson(source: string, retainNumberTokens: boolean): NativeValue {
  // Parse every token, including values overwritten by duplicate keys, before comparing objects.
  const tokens: RegExp = /[ \t\r\n]*(?:-?(0|[1-9]\d*)(?:\.(\d+))?(?:[eE]([+-]?\d+))?|true|false|null|["{}\[\],:])/y;
  const stringDelimiters: RegExp = /["\\]/g;
  let next: RegExpExecArray | null = tokens.exec(source);
  let consumed: number = 0;
  function take(): string {
    if (!next) throw new SyntaxError('Invalid JSON argument.');
    const prefix: string = next[0].trimStart();
    const start: number = tokens.lastIndex - prefix.length;
    if (prefix === '"') {
      // Let JSON.parse validate the string once its unescaped closing quote is located.
      stringDelimiters.lastIndex = tokens.lastIndex;
      let delimiter: RegExpExecArray | null = stringDelimiters.exec(source);
      while (delimiter?.[0] === '\\') {
        stringDelimiters.lastIndex++;
        delimiter = stringDelimiters.exec(source);
      }
      if (!delimiter) throw new SyntaxError('Unterminated JSON string.');
      tokens.lastIndex = stringDelimiters.lastIndex;
    }

    consumed = tokens.lastIndex;
    next = tokens.exec(source);
    return source.slice(start, consumed);
  }

  function takeIf(token: string): boolean {
    if (next?.[0].trimStart() !== token) return false;
    take();
    return true;
  }

  function readChild(parent: object, key: string, depth: number): NativeValue {
    const raw: string | undefined = retainNumberTokens && next?.[1] !== undefined ? next[0].trimStart() : undefined;
    const value: NativeValue = read(depth);
    if (typeof value === 'number' && raw !== undefined) rememberNumberSource(parent, key, value, raw);
    return value;
  }

  function read(depth: number): NativeValue {
    const match: RegExpExecArray | null = next;
    const token: string = take();
    if (match?.[1] !== undefined) {
      const value: bigint | number = nativeNumber(token.startsWith('-'), match[1], match[2], match[3]);
      return retainNumberTokens ? Number(value) : value;
    }
    if (token === 'null') return null;
    if (token === 'true' || token === 'false') return token === 'true';
    if (token.startsWith('"')) return nativeString(token);
    if (depth === 127) throw new SyntaxError('JSON arguments exceed the native nesting limit.');
    if (token === '[') {
      const values: NativeValue[] = [];
      if (takeIf(']')) return values;
      do { values.push(readChild(values, String(values.length), depth + 1)); } while (takeIf(','));
      if (takeIf(']')) return values;
    }

    if (token === '{') {
      const values: Record<string, NativeValue> = Object.create(null) as Record<string, NativeValue>;
      if (takeIf('}')) return values;
      do {
        const key: string = nativeString(take());
        if (!takeIf(':')) throw new SyntaxError('Invalid JSON object.');
        values[key] = readChild(values, key, depth + 1);
      } while (takeIf(','));
      if (takeIf('}')) return values;
    }

    throw new SyntaxError('Invalid JSON argument.');
  }

  const value: NativeValue = read(0);
  if (/[^ \t\r\n]/.test(source.slice(consumed))) throw new SyntaxError('Invalid JSON argument.');
  return value;
}

function nativeString(token: string): string {
  if (!token.startsWith('"')) throw new SyntaxError('Expected a JSON string.');
  const value: string = JSON.parse(token) as string;
  if (/[\uD800-\uDFFF]/u.test(value)) throw new SyntaxError('Invalid Unicode in JSON argument.');
  return value;
}

function readSignificand(digits: string, initial: bigint): { value: bigint; consumed: number } {
  let value: bigint = initial;
  let consumed: number = initial === 0n ? digits.search(/[1-9]/) : 0;
  if (consumed === -1) return { value, consumed: digits.length };
  for (; consumed < digits.length; consumed++) {
    const updated: bigint = value * 10n + BigInt(digits.charCodeAt(consumed) - 48);
    if (updated > MAX_U64) break;
    value = updated;
  }
  return { value, consumed };
}

function nativeNumber(negative: boolean, whole: string, fraction?: string, exponent?: string): bigint | number {
  const integer: { value: bigint; consumed: number } = readSignificand(whole, 0n);
  if (fraction === undefined && exponent === undefined && integer.consumed === whole.length) {
    const signed: bigint = negative ? -integer.value : integer.value;
    if (!negative || (signed < 0n && signed >= MIN_I64)) return signed;
  }

  // serde_json rounds the u64 significand before applying its decimal exponent.
  const decimal: { value: bigint; consumed: number } = readSignificand(fraction ?? '', integer.value);
  const explicitExponent: number = Number(exponent ?? 0);
  if (explicitExponent > MAX_I32 && decimal.value !== 0n) throw new SyntaxError('JSON number out of range.');
  if (decimal.value === 0n || explicitExponent < -MAX_I32) return 0;
  let power: number = whole.length - integer.consumed - decimal.consumed + explicitExponent;
  let value: number = Number(decimal.value);
  while (power < -308 && value !== 0) {
    value /= 1e308;
    power += 308;
  }

  const factor: number = Number(`1e${Math.abs(power)}`);
  value = power < 0 ? value / factor : value * factor;
  if (!Number.isFinite(value)) throw new SyntaxError('JSON number out of range.');
  // serde_json treats positive and negative floating-point zero as equal.
  return value === 0 ? 0 : negative ? -value : value;
}
