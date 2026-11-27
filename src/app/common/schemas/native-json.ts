/** Decode the same JSON values as the native router's default serde_json parser. */
import { isDeepStrictEqual } from 'node:util';
import { numberSource, rememberNumberSource } from './json-wire.ts';
import { MAX_U64 } from './token-count.ts';

/** A directly deserialized Rust struct or vector, with undeclared struct fields ignored. */
export type NativeJsonShape = { [field: string]: NativeJsonShape | true } | readonly [NativeJsonShape | true];

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

/** Decode for validation or transport with ordinary JS numbers and exact wire tokens. */
export function parseNativeJson(source: string | Uint8Array, shape?: NativeJsonShape): unknown {
  if (typeof source === 'string') return readNativeJson(source, true, shape);
  const decoder: TextDecoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
  let text: string;
  try { text = decoder.decode(source); }
  catch (error: unknown) {
    if (!shape) throw error;
    // Directly deserialized structs can skip byte strings in undeclared fields.
    // Only strings the parser reads as values or field names need valid UTF-8.
    return readNativeJson(Buffer.from(source).toString('latin1'), true, shape, decoder);
  }
  return readNativeJson(text, true, shape);
}

/** Read a native Value::Number as u64 without rounding, including when JS represents it as a float. */
export function nativeUnsignedField(value: unknown, key: string): bigint | undefined {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) return undefined;
  const field: unknown = (value as Record<string, unknown>)[key];
  if (typeof field !== 'number') return undefined;
  const source: string = numberSource(value, key, field);
  if (!/^[0-9]{1,20}$/.test(source)) return undefined;
  const integer: bigint = BigInt(source);
  return integer <= MAX_U64 ? integer : undefined;
}

/** Recover the serde_json number representation for a decoded schema or instance field. */
export function nativeSchemaNumber(value: number, parent?: object, key?: string | number): bigint | number {
  const source: string = parent && key !== undefined ? numberSource(parent, String(key), value) : String(value);
  if (value === 0 && source.startsWith('-')) return -0;
  if (!/^(?:0|-?[1-9][0-9]{0,19})$/.test(source)) return value;
  const integer: bigint = BigInt(source);
  return integer >= MIN_I64 && integer <= MAX_U64 ? integer : value;
}

function readNativeJson(source: string, retainNumberTokens: boolean, shape?: NativeJsonShape, byteStrings?: TextDecoder): NativeValue {
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

  function readChild(parent: object, key: string, depth: number, childShape: NativeJsonShape | true = true): NativeValue {
    const raw: string | undefined = retainNumberTokens && next?.[1] !== undefined ? next[0].trimStart() : undefined;
    const value: NativeValue = read(depth, childShape);
    if (typeof value === 'number' && raw !== undefined) rememberNumberSource(parent, key, value, raw);
    return value;
  }

  function read(depth: number, node: NativeJsonShape | true): NativeValue {
    const match: RegExpExecArray | null = next;
    const token: string = take();
    if (match?.[1] !== undefined) {
      const value: bigint | number = nativeNumber(token.startsWith('-'), match[1], match[2], match[3]);
      return retainNumberTokens ? Number(value) : value;
    }
    if (token === 'null') return null;
    if (token === 'true' || token === 'false') return token === 'true';
    if (token.startsWith('"')) return nativeString(token, byteStrings);
    if (depth === 127) throw new SyntaxError('JSON arguments exceed the native nesting limit.');
    if (token === '[') {
      const values: NativeValue[] = [];
      if (takeIf(']')) return values;
      const fields: (NativeJsonShape | true)[] | undefined = node === true ? undefined : Object.values(node);
      do {
        const field: NativeJsonShape | true = Array.isArray(node) ? node[0] : fields?.[values.length] ?? true;
        values.push(readChild(values, String(values.length), depth + 1, field));
      } while (takeIf(','));
      if (takeIf(']')) return values;
    }

    if (token === '{') {
      const values: Record<string, NativeValue> = Object.create(null) as Record<string, NativeValue>;
      if (takeIf('}')) return values;
      do {
        const key: string = nativeString(take(), byteStrings);
        if (!takeIf(':')) throw new SyntaxError('Invalid JSON object.');
        if (node !== true && !Array.isArray(node)) {
          if (!Object.hasOwn(node, key)) {
            skipValue();
            continue;
          }
          if (Object.hasOwn(values, key)) throw new SyntaxError('Duplicate JSON field.');
          values[key] = readChild(values, key, depth + 1, (node as Record<string, NativeJsonShape | true>)[key]);
        } else {
          values[key] = readChild(values, key, depth + 1);
        }
      } while (takeIf(','));
      if (takeIf('}')) return values;
    }

    throw new SyntaxError('Invalid JSON argument.');
  }

  // An ignored field only needs JSON syntax, including when its value cannot form a native Value.
  function skipValue(): void {
    const start: number = consumed;
    let nesting: number = 0;
    do {
      const token: string = take();
      if (token === '{' || token === '[') nesting++;
      if (token === '}' || token === ']') nesting--;
    } while (nesting > 0);
    const skipped: string = source.slice(start, consumed);
    JSON.parse(skipped);
  }

  const value: NativeValue = read(0, shape ?? true);
  if (/[^ \t\r\n]/.test(source.slice(consumed))) throw new SyntaxError('Invalid JSON argument.');
  return value;
}

function nativeString(token: string, byteStrings?: TextDecoder): string {
  if (!token.startsWith('"')) throw new SyntaxError('Expected a JSON string.');
  const value: string = JSON.parse(byteStrings ? byteStrings.decode(Buffer.from(token, 'latin1')) : token) as string;
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
