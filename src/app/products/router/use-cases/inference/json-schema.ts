/** Select the native JSON Schema dialect for inference contracts and provider output. */
import { Ajv, type Options, type ValidateFunction } from 'ajv';
import AjvDraft04 from 'ajv-draft-04';
import { Ajv2019 } from 'ajv/dist/2019.js';
import { Ajv2020 } from 'ajv/dist/2020.js';
import draft4Meta from 'ajv-draft-04/dist/refs/json-schema-draft-04.json' with { type: 'json' };
import draft6 from 'ajv/dist/refs/json-schema-draft-06.json' with { type: 'json' };
import draft7 from 'ajv/dist/refs/json-schema-draft-07.json' with { type: 'json' };
import { copyNumberSources, rememberNumberSource } from '../../../../common/schemas/json-wire.ts';
import { parseNativeJson } from '../../../../common/schemas/native-json.ts';
import { useNativeNumbers } from './schema-numbers.ts';

const draft4Uri: string = 'http://json-schema.org/draft-04/schema';
const draft6Uri: string = 'http://json-schema.org/draft-06/schema';
const draft7Uri: string = 'http://json-schema.org/draft-07/schema';
const draft2019Uri: string = 'https://json-schema.org/draft/2019-09/schema';
const draft2020Uri: string = 'https://json-schema.org/draft/2020-12/schema';
const localBase: string = 'json-schema:///';
type SchemaPositions = { maps: readonly string[]; values: readonly string[]; arrays: readonly string[] };
const commonMaps: readonly string[] = ['properties', 'patternProperties', 'definitions'];
const commonArrays: readonly string[] = ['allOf', 'anyOf', 'oneOf'];
const legacyMaps: readonly string[] = [...commonMaps, 'dependencies'];
const modernMaps: readonly string[] = [...commonMaps, '$defs', 'dependentSchemas'];
const commonValues: readonly string[] = ['additionalProperties', 'contains', 'not', 'propertyNames'];
const draft6Values: readonly string[] = [...commonValues, 'additionalItems'];
const draft7Values: readonly string[] = [...draft6Values, 'if', 'then', 'else'];
const modernValues: readonly string[] = [...commonValues, 'if', 'then', 'else', 'contentSchema', 'unevaluatedItems', 'unevaluatedProperties'];
const schemaMaps: readonly string[] = [...modernMaps, 'dependencies'];
const schemaValues: readonly string[] = [...modernValues, ...commonArrays, 'additionalItems', 'items', 'prefixItems'];
const draft2020Positions: SchemaPositions = { maps: modernMaps, arrays: [...commonArrays, 'prefixItems'], values: modernValues };
// jsonschema 0.30 discovers resource IDs in these positions, including legacy annotations in Draft 4.
const resourcePositions: ReadonlyMap<string, SchemaPositions> = new Map([
  [draft4Uri, { maps: schemaMaps, arrays: draft2020Positions.arrays, values: [...modernValues, 'additionalItems'] }],
  [draft6Uri, { maps: legacyMaps, arrays: commonArrays, values: draft6Values }],
  [draft7Uri, { maps: legacyMaps, arrays: commonArrays, values: draft7Values }],
  [draft2019Uri, { maps: modernMaps, arrays: commonArrays, values: [...modernValues, 'additionalItems'] }],
  [draft2020Uri, draft2020Positions],
]);
const options: Options = {
  strict: false, validateFormats: false, validateSchema: false, addUsedSchema: false, ownProperties: true, logger: false,
};
const legacyOptions: Options = { ...options, meta: false, ignoreKeywordsWithRef: true };
const nativeDraft4ByNode: WeakMap<object, boolean> = new WeakMap();
const usesDraft4: (schema: object) => boolean | undefined = (schema: object): boolean | undefined => nativeDraft4ByNode.get(schema);
const draft4: Ajv = new AjvDraft04.default(legacyOptions).addMetaSchema(legacyMeta(draft4Meta, { format: { type: 'string' } }));
const draft6Validator: Ajv = useLegacyContent(new Ajv(legacyOptions).addMetaSchema(legacyMeta(draft6)));
const draft7Validator: Ajv = useLegacyContent(new Ajv(legacyOptions).addMetaSchema(legacyMeta(draft7, { writeOnly: { type: 'boolean' } })));
const drafts: ReadonlyMap<string, Ajv> = new Map([
  [draft4Uri, useNativeNumbers(draft4.removeKeyword('contains').removeKeyword('propertyNames')
    .removeKeyword('if').removeKeyword('then').removeKeyword('else'), true, usesDraft4)],
  [draft6Uri, useNativeNumbers(draft6Validator.removeKeyword('if').removeKeyword('then').removeKeyword('else'), false, usesDraft4)],
  [draft7Uri, useNativeNumbers(draft7Validator, false, usesDraft4)],
  [draft2019Uri, useNativeNumbers(new Ajv2019(options)
    .removeKeyword('id').removeKeyword('$dynamicRef').removeKeyword('$dynamicAnchor'), false, usesDraft4)],
  [draft2020Uri, useNativeNumbers(new Ajv2020(options)
    .removeKeyword('id').removeKeyword('$recursiveRef').removeKeyword('$recursiveAnchor'), false, usesDraft4)],
]);
const compiled: WeakMap<Record<string, unknown>, ValidateFunction> = new WeakMap();

/** Compile a local schema with the same draft detection and meta-validation as native. */
export function compileJsonSchema(schema: Record<string, unknown>): ValidateFunction {
  const cached: ValidateFunction | undefined = compiled.get(schema);
  if (cached) return cached;

  const declared: unknown = schema.$schema;
  const uri: string = typeof declared === 'string' ? declared.replace(/#$/, '') : draft2020Uri;
  const ajv: Ajv | undefined = drafts.get(uri);
  if (!ajv || (declared !== undefined && typeof declared !== 'string')) throw new Error('Unsupported JSON Schema dialect.');

  // Native accepts an optional empty URI fragment and validates against the canonical meta-schema.
  if (ajv.getSchema(uri)?.(schema) !== true) throw new Error('Invalid JSON Schema.');
  // Keep Ajv's strong cache bounded. Active requests hold their compiled validator in a weak map.
  ajv.removeSchema();
  checkSchemaResources(ajv, schema, uri);
  const prepared: Record<string, unknown> = prepareSchema(schema, uri, true);
  const validate: ValidateFunction = ajv.compile(prepared);
  compiled.set(schema, validate);
  return validate;
}

/** Validate native JSON while making a scalar root's wire number available to Ajv keywords. */
export function matchesJsonSchema(schema: Record<string, unknown>, source: string): boolean {
  try {
    const value: unknown = parseNativeJson(source);
    const root: { value: unknown } = { value };
    if (typeof value === 'number') rememberNumberSource(root, 'value', value, source.trim());
    return compileJsonSchema(schema)(value, {
      instancePath: '', parentData: root, parentDataProperty: 'value',
      rootData: value as Record<string, unknown>, dynamicAnchors: {},
    }) === true;
  } catch {
    return false;
  }
}

function prepareSchema(schema: Record<string, unknown>, enclosingUri: string, root: boolean = false): Record<string, unknown> {
  const uri: string = typeof schema.$schema === 'string' ? schema.$schema.replace(/#$/, '') : enclosingUri;
  const legacyRef: boolean = uri === draft4Uri || uri === draft6Uri || uri === draft7Uri;
  // Native omits a direct reference to the same root. Recursive child references still apply.
  if (root && legacyRef && schema.$ref === '#') return {};
  const result: Record<string, unknown> = { ...schema };
  copyNumberSources(result, schema);
  nativeDraft4ByNode.set(result, uri === draft4Uri);
  // Ajv extensions are annotations to native. Legacy refs also suppress sibling type validation.
  delete result.nullable;
  delete result.$async;
  if (legacyRef && typeof result.$ref === 'string') delete result.type;
  if (root && result.$ref === '#') delete result.$ref;

  // Walk only schema positions. Literal objects inside const, enum and examples keep their JSON values.
  for (const key of schemaMaps) {
    const children: unknown = result[key];
    if (isSchemaObject(children)) {
      result[key] = Object.fromEntries(Object.entries(children).map(([name, child]) =>
        [name, isSchemaObject(child) ? prepareSchema(child, uri) : child]));
    }
  }
  for (const key of schemaValues) {
    const child: unknown = result[key];
    if (isSchemaObject(child)) result[key] = prepareSchema(child, uri);
    else if (Array.isArray(child)) result[key] = child.map((item: unknown): unknown =>
      isSchemaObject(item) ? prepareSchema(item, uri) : item);
  }
  return result;
}

function checkSchemaResources(ajv: Ajv, schema: Record<string, unknown>, uri: string): void {
  const resources: Set<string> = new Set();
  const references: Set<string> = new Set();
  const resolve = ajv.opts.uriResolver.resolve;
  const visit = (node: unknown, parentUri: string, parentBase: string): void => {
    if (!isSchemaObject(node)) return;
    const declared: string | undefined = typeof node.$schema === 'string' ? withoutFragment(node.$schema) : undefined;
    const dialect: string = declared && drafts.has(declared) ? declared : parentUri;
    const legacy: boolean = dialect === draft4Uri || dialect === draft6Uri || dialect === draft7Uri;
    const id: unknown = legacy && typeof node.$ref === 'string' ? undefined : node[dialect === draft4Uri ? 'id' : '$id'];
    const base: string = typeof id === 'string' ? withoutFragment(resolve(parentBase, id)) : parentBase;
    resources.add(base);
    if (typeof node.$ref === 'string') references.add(withoutFragment(resolve(base, node.$ref)));
    if (declared !== undefined) references.add(withoutFragment(resolve(base, declared)));

    const positions: SchemaPositions = resourcePositions.get(dialect) ?? draft2020Positions;
    for (const key of positions.maps) {
      const children: unknown = node[key];
      if (isSchemaObject(children)) for (const child of Object.values(children)) visit(child, dialect, base);
    }
    for (const key of positions.values) visit(node[key], dialect, base);
    for (const key of positions.arrays) {
      const children: unknown = node[key];
      if (Array.isArray(children)) for (const item of children) visit(item, dialect, base);
    }
    if (dialect !== draft2020Uri && Array.isArray(node.items)) {
      for (const item of node.items) visit(item, dialect, base);
    } else {
      visit(node.items, dialect, base);
    }
  };
  visit(schema, uri, localBase);
  for (const reference of references) {
    // Native discovers references even under unused definitions. Only embedded resources and built-in meta-schemas are available.
    if (resources.has(reference) || drafts.has(reference) || ajv.getSchema(reference)) continue;
    throw new Error('Unresolvable JSON Schema resource.');
  }
}

function withoutFragment(uri: string): string {
  return uri.split('#', 1)[0];
}

function isSchemaObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function legacyMeta(meta: { properties: Record<string, unknown> }, properties?: Record<string, unknown>): Record<string, unknown> {
  // The crate's legacy meta-schemas accept empty enum arrays and duplicate members.
  return { ...meta, properties: { ...meta.properties, ...properties, enum: { type: 'array' } } };
}

function useLegacyContent(ajv: Ajv): Ajv {
  // Native asserts these content keywords in Draft 6 and 7. Later drafts treat them as annotations.
  return ajv.removeKeyword('id').removeKeyword('contentEncoding').addKeyword({
    keyword: 'contentEncoding', type: 'string', errors: false,
    compile: (encoding: unknown, schema: Record<string, unknown>) => {
      if (schema.contentMediaType !== undefined) return (): boolean => true;
      if (typeof encoding !== 'string') throw new Error('Invalid content encoding.');
      return encoding === 'base64' ? (value: string): boolean => decodeBase64(value) !== undefined : (): boolean => true;
    },
  }).removeKeyword('contentMediaType').addKeyword({
    keyword: 'contentMediaType', type: 'string', errors: false,
    compile: (mediaType: unknown, schema: Record<string, unknown>) => {
      if (typeof mediaType !== 'string') throw new Error('Invalid content media type.');
      if (mediaType !== 'application/json') return (): boolean => true;
      if (schema.contentEncoding === undefined) return isJson;
      if (typeof schema.contentEncoding !== 'string') throw new Error('Invalid content encoding.');
      if (schema.contentEncoding !== 'base64') return (): boolean => true;
      return (value: string): boolean => {
        const bytes: Buffer | undefined = decodeBase64(value);
        return bytes !== undefined && isJson(bytes);
      };
    },
  });
}

function decodeBase64(value: string): Buffer | undefined {
  const bytes: Buffer = Buffer.from(value, 'base64');
  return bytes.toString('base64') === value ? bytes : undefined;
}

function isJson(source: string | Uint8Array): boolean {
  try {
    parseNativeJson(source);
    return true;
  } catch {
    return false;
  }
}
