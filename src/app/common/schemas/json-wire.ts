/** Retain JSON number tokens when opaque provider state crosses the HTTP boundary. */
interface NumberToken { source: string; value: number }
const numberSources: WeakMap<object, Map<string, NumberToken>> = new WeakMap();
const json: JSON & { rawJSON(text: string): object } = JSON as JSON & { rawJSON(text: string): object };

export function rememberNumberSource(parent: object, key: string, value: number, source: string): void {
  const fields: Map<string, NumberToken> | undefined = numberSources.get(parent);
  if (source === String(value)) fields?.delete(key);
  else if (fields) fields.set(key, { source, value });
  else numberSources.set(parent, new Map([[key, { source, value }]]));
}

export function numberSource(parent: object, key: string, value: number): string {
  const token: NumberToken | undefined = numberSources.get(parent)?.get(key);
  return token && Object.is(token.value, value) ? token.source : String(value);
}

/** Transfer source tokens when selected opaque fields are copied or overlaid. */
export function copyNumberSources(target: object, source: object): void {
  const prior: Map<string, NumberToken> | undefined = numberSources.get(target);
  for (const key of Object.keys(source)) prior?.delete(key);
  const fields: Map<string, NumberToken> | undefined = numberSources.get(source);
  if (!fields) return;
  const copied: Map<string, NumberToken> = prior ?? new Map();
  for (const [key, token] of fields) if (Object.hasOwn(target, key)) copied.set(key, token);
  numberSources.set(target, copied);
}

/** Node 24 emits wide counters and opaque numeric values without narrowing their JSON tokens. */
export function stringifyWireJson(value: unknown, space?: number): string {
  return json.stringify(value, function (this: object, key: string, entry: unknown): unknown {
    if (typeof entry === 'bigint') return json.rawJSON(entry.toString());
    const token: NumberToken | undefined = typeof entry === 'number' ? numberSources.get(this)?.get(key) : undefined;
    return token && Object.is(token.value, entry) ? json.rawJSON(token.source) : entry;
  }, space);
}
