/** Adapt serde struct sequences and native JSON number tokens before Zod validates DTO fields. */
import { z } from 'zod';
import { numberSource, rememberNumberSource } from './json-wire.ts';
import { nativeUnsignedField } from './native-json.ts';
import { tokenCount } from './token-count.ts';

export function nativeStruct<T extends z.ZodObject>(
  schema: T,
  fields: readonly string[] = Object.keys(schema.shape),
  minimum: number = fields.length,
  unsigned: readonly string[] = [],
) {
  return z.preprocess((value: unknown): unknown => nativeStructObject(value, fields, minimum),
    unsigned.length ? nativeUnsignedFields(schema, unsigned) : schema);
}

export function nativeStructObject(value: unknown, fields: readonly string[], minimum: number = fields.length): unknown {
  if (!Array.isArray(value) || value.length < minimum || value.length > fields.length) return value;
  const object: Record<string, unknown> = Object.create(null) as Record<string, unknown>;
  for (const [index, entry] of value.entries()) {
    const field: string = fields[index]!;
    object[field] = entry;
    if (typeof entry === 'number') rememberNumberSource(object, field, entry, numberSource(value, String(index), entry));
  }
  return object;
}

/** Decode u64 before object validation strips the original JSON number tokens. */
export function nativeUnsignedFields<T extends z.ZodType>(schema: T, fields: readonly string[]) {
  return z.preprocess((value: unknown): unknown => {
    if (value === null || typeof value !== 'object' || Array.isArray(value)) return value;
    const object: Record<string, unknown> = { ...value };
    for (const field of fields) {
      if (typeof object[field] === 'number') {
        const integer: bigint | undefined = nativeUnsignedField(value, field);
        object[field] = integer === undefined ? NaN : tokenCount(integer);
      }
    }
    return object;
  }, schema);
}

/** An omitted or null serde Option is absent from the normalized request. */
export function nativeOption<T extends z.ZodType>(schema: T) {
  return schema.nullable().transform((value): z.output<T> | undefined => value ?? undefined).optional();
}
