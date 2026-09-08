import { spawnSync } from 'node:child_process'
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, resolve } from 'node:path'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const exported = spawnSync('cargo', ['run', '--offline', '--quiet', '--manifest-path', resolve(root, 'crates/fox-engine-protocol/Cargo.toml'), '--bin', 'export-schema'], {
  cwd: root, encoding: 'utf8', windowsHide: true,
})
if (exported.status !== 0) throw new Error(exported.error?.message ?? exported.stderr ?? 'protocol schema export failed')
const bundle = JSON.parse(exported.stdout)
const destination = resolve(root, 'packages/fox-engine-protocol')
const banner = '// Generated from fox-engine-protocol Rust DTOs. Do not edit; run node scripts/generate-engine-protocol.mjs.\n'

function typeOf(schema) {
  if (schema === true) return 'unknown'
  if (schema === false) return 'never'
  if (schema.$ref) return schema.$ref.split('/').at(-1)
  if ('const' in schema) return JSON.stringify(schema.const)
  if (schema.enum) return schema.enum.map(value => JSON.stringify(value)).join(' | ')
  if (schema.anyOf || schema.oneOf) return '(' + (schema.anyOf ?? schema.oneOf).map(typeOf).join(' | ') + ')'
  if (schema.allOf) return '(' + schema.allOf.map(typeOf).join(' & ') + ')'
  if (Array.isArray(schema.type)) return '(' + schema.type.map(type => typeOf({ ...schema, type })).join(' | ') + ')'
  switch (schema.type) {
    case 'null': return 'null'
    case 'string': return 'string'
    case 'number': case 'integer': return 'number'
    case 'boolean': return 'boolean'
    case 'array': return 'Array<' + typeOf(schema.items ?? true) + '>'
    case 'object': {
      const fields = Object.entries(schema.properties ?? {}).map(([name, field]) =>
        JSON.stringify(name) + (schema.required?.includes(name) ? '' : '?') + ': ' + typeOf(field) + ';')
      if (!fields.length) return 'Record<string, ' + typeOf(schema.additionalProperties ?? true) + '>'
      return '{ ' + fields.join(' ') + ' }'
    }
    case undefined: return 'unknown'
    default: throw new Error('unsupported schema type: ' + schema.type)
  }
}

// Reject new validation constructs until the shared validator supports them.
const supported = new Set(['$schema', '$id', '$ref', '$defs', 'title', 'description', 'default', 'examples', 'deprecated',
  'readOnly', 'writeOnly', 'type', 'const', 'enum', 'anyOf', 'oneOf', 'allOf', 'properties', 'required',
  'additionalProperties', 'items', 'minimum', 'maximum', 'minLength', 'maxLength', 'minItems', 'maxItems', 'pattern', 'format'])
function checkSchema(schema) {
  if (typeof schema === 'boolean') return
  for (const key of Object.keys(schema)) if (!supported.has(key)) throw new Error('unsupported schema keyword: ' + key)
  for (const child of Object.values(schema.$defs ?? {})) checkSchema(child)
  for (const child of Object.values(schema.properties ?? {})) checkSchema(child)
  for (const child of [...(schema.anyOf ?? []), ...(schema.oneOf ?? []), ...(schema.allOf ?? [])]) checkSchema(child)
  if (schema.items) checkSchema(schema.items)
  if (typeof schema.additionalProperties === 'object') checkSchema(schema.additionalProperties)
}

const definitions = new Map()
for (const [name, schema] of Object.entries(bundle.schemas)) {
  checkSchema(schema)
  for (const [ref, definition] of Object.entries(schema.$defs ?? {})) {
    if (definitions.has(ref) && JSON.stringify(definitions.get(ref)) !== JSON.stringify(definition)) {
      throw new Error('conflicting generated type definition: ' + ref)
    }
    definitions.set(ref, definition)
  }
  definitions.set(name, schema)
}
const declarations = banner + Array.from(definitions, ([name, schema]) => 'export type ' + name + ' = ' + typeOf(schema) + '\n').join('')
  + '\nexport declare const PROTOCOL_NAME: ' + JSON.stringify(bundle.protocolName) + '\n'
  + 'export declare const PROTOCOL_VERSION: ' + bundle.protocolVersion + '\n'
  + 'export declare const CAPABILITY_MANIFEST_VERSION: ' + bundle.capabilityManifestVersion + '\n'
  + 'export declare const RUNTIME_TOOL_CATALOG: ReadonlyArray<Readonly<RuntimeToolCapability>>\n'
  + 'export declare function validateWireValue(schemaName: string, value: unknown): string[]\n'
const module = banner
  + "import { validateSchema } from './validate.mjs'\n"
  + 'export const SCHEMA_BUNDLE = ' + JSON.stringify(bundle, null, 2) + '\n'
  + 'export const PROTOCOL_NAME = SCHEMA_BUNDLE.protocolName\n'
  + 'export const PROTOCOL_VERSION = SCHEMA_BUNDLE.protocolVersion\n'
  + 'export const CAPABILITY_MANIFEST_VERSION = SCHEMA_BUNDLE.capabilityManifestVersion\n'
  + 'export const RUNTIME_TOOL_CATALOG = Object.freeze(SCHEMA_BUNDLE.toolContracts.map(Object.freeze))\n'
  + 'export function validateWireValue(schemaName, value) {\n'
  + '  const schema = SCHEMA_BUNDLE.schemas[schemaName]\n'
  + "  return schema ? validateSchema(schema, value) : ['unknown wire schema: ' + schemaName]\n}\n"
const outputs = new Map([['schema.json', JSON.stringify(bundle, null, 2) + '\n'], ['index.d.ts', declarations], ['index.mjs', module]])
const checkOnly = process.argv.includes('--check')
if (!checkOnly) mkdirSync(destination, { recursive: true })
for (const [name, content] of outputs) {
  const path = resolve(destination, name)
  if (checkOnly) {
    let actual
    try { actual = readFileSync(path, 'utf8') } catch { throw new Error('missing generated protocol file: ' + name) }
    if (actual !== content) throw new Error('stale generated protocol file: ' + name)
  } else writeFileSync(path, content, 'utf8')
}
console.log(checkOnly ? 'Engine protocol bindings are current.' : 'Generated engine protocol schema and bindings.')
