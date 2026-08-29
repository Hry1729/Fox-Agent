import { createHash } from 'node:crypto'

export const TYPED_CONTEXT_SCHEMA_VERSION = 1

const ID_PATTERN = /^[a-z][a-z0-9_]{0,63}$/u
const HASH_PATTERN = /^[a-f0-9]{64}$/u
const LIFECYCLES = new Set(['turn', 'session', 'persistent'])
const CONTEXT_MARKER_START = /<(?=\s*(?:\\*\/\s*)?fox_context_block\b)/giu

const KIND_CONTRACTS = Object.freeze({
  assistant_persona: Object.freeze({ authority: 'runtime', trust: 'runtime_verified' }),
  runtime: Object.freeze({ authority: 'runtime', trust: 'runtime_verified' }),
  expert_package: Object.freeze({ authority: 'runtime', trust: 'runtime_verified' }),
  expert_binding: Object.freeze({ authority: 'runtime', trust: 'runtime_verified' }),
  confirmed_memory: Object.freeze({ authority: 'runtime', trust: 'runtime_verified' }),
  workspace: Object.freeze({ authority: 'workspace', trust: 'workspace_declared' }),
  work_snapshot: Object.freeze({ authority: 'host', trust: 'host_verified' }),
  turn_tail: Object.freeze({ authority: 'runtime', trust: 'runtime_verified' }),
  planner_handoff: Object.freeze({ authority: 'runtime', trust: 'runtime_verified' }),
  approval_demo: Object.freeze({ authority: 'runtime', trust: 'runtime_verified' }),
  user_message: Object.freeze({ authority: 'user', trust: 'user_supplied' }),
  untrusted_evidence: Object.freeze({ authority: 'untrusted', trust: 'untrusted' }),
})

const ROOT_KEYS = Object.freeze([
  'schemaVersion',
  'id',
  'kind',
  'authority',
  'trust',
  'version',
  'hash',
  'budget',
  'lifecycle',
  'content',
])
const BUDGET_KEYS = Object.freeze(['priority', 'minimumChars', 'maxChars'])

function canonicalJson(value) {
  if (value === null || typeof value !== 'object') return JSON.stringify(value)
  if (Array.isArray(value)) return `[${value.map((item) => canonicalJson(item)).join(',')}]`
  return `{${Object.keys(value)
    .filter((key) => value[key] !== undefined)
    .sort((left, right) => left.localeCompare(right))
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(',')}}`
}

function sha256(value) {
  return createHash('sha256').update(String(value), 'utf8').digest('hex')
}

function deepFreeze(value) {
  if (!value || typeof value !== 'object' || Object.isFrozen(value)) return value
  for (const child of Object.values(value)) deepFreeze(child)
  return Object.freeze(value)
}

function exactKeys(value, allowed, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`[typed_context.invalid_${label}] ${label} must be an object.`)
  }
  const prototype = Object.getPrototypeOf(value)
  if (prototype !== Object.prototype && prototype !== null) {
    throw new Error(`[typed_context.invalid_${label}_prototype] ${label} must use a plain object prototype.`)
  }
  const allowedKeys = new Set(allowed)
  const unknown = Object.keys(value).filter((key) => !allowedKeys.has(key))
  if (unknown.length > 0) {
    throw new Error(`[typed_context.unknown_${label}_key] Unsupported ${label} keys: ${unknown.join(', ')}`)
  }
}

function positiveInteger(value, label, { allowZero = false } = {}) {
  if (!Number.isInteger(value) || (allowZero ? value < 0 : value < 1)) {
    throw new Error(`[typed_context.invalid_${label}] ${label} must be ${allowZero ? 'a non-negative' : 'a positive'} integer.`)
  }
  return value
}

function normalizeBudget(budget = {}) {
  exactKeys(budget, BUDGET_KEYS, 'budget')
  const priority = positiveInteger(budget.priority ?? 50, 'budget_priority')
  const minimumChars = positiveInteger(budget.minimumChars ?? 0, 'budget_minimum_chars', { allowZero: true })
  const maxChars = positiveInteger(budget.maxChars ?? 18_000, 'budget_max_chars')
  if (minimumChars > maxChars) {
    throw new Error('[typed_context.invalid_budget] minimumChars cannot exceed maxChars.')
  }
  return { priority, minimumChars, maxChars }
}

function hashInput(fragment) {
  return {
    schemaVersion: fragment.schemaVersion,
    id: fragment.id,
    kind: fragment.kind,
    authority: fragment.authority,
    trust: fragment.trust,
    version: fragment.version,
    budget: fragment.budget,
    lifecycle: fragment.lifecycle,
    content: fragment.content,
  }
}

export function typedContextHash(value) {
  return sha256(canonicalJson(value))
}

export function typedContextSchemaHash() {
  return typedContextHash({
    schemaVersion: TYPED_CONTEXT_SCHEMA_VERSION,
    rootKeys: ROOT_KEYS,
    budgetKeys: BUDGET_KEYS,
    lifecycles: [...LIFECYCLES].sort(),
    kinds: Object.fromEntries(Object.entries(KIND_CONTRACTS).sort(([left], [right]) => left.localeCompare(right))),
  })
}

export function supportedTypedContextKinds() {
  return Object.keys(KIND_CONTRACTS)
}

export function neutralizeTypedContextMarkers(content) {
  // Keep the content character length stable so the exact rendered representation can
  // be budgeted before composition. Replacing only the opening angle bracket also
  // preserves the surrounding code/JSON as readable evidence instead of deleting it.
  return String(content).replace(CONTEXT_MARKER_START, '[')
}

export function createTypedContextFragment(input) {
  if (typeof input === 'string') {
    throw new Error('[typed_context.raw_string_forbidden] Context must be supplied as a typed fragment object.')
  }
  if (!input || typeof input !== 'object' || Array.isArray(input)) {
    throw new Error('[typed_context.invalid_fragment] Typed context fragment must be an object.')
  }
  const allowedInputKeys = ROOT_KEYS.filter((key) => key !== 'hash')
  exactKeys(input, allowedInputKeys, 'fragment')
  const schemaVersion = input.schemaVersion ?? TYPED_CONTEXT_SCHEMA_VERSION
  if (schemaVersion !== TYPED_CONTEXT_SCHEMA_VERSION) {
    throw new Error(`[typed_context.unsupported_schema] Unsupported typed context schema version: ${String(schemaVersion)}`)
  }
  const id = String(input.id ?? '').trim()
  if (!ID_PATTERN.test(id)) throw new Error(`[typed_context.invalid_id] Invalid typed context fragment id: ${id || '<empty>'}`)
  const kind = String(input.kind ?? '').trim()
  if (!Object.hasOwn(KIND_CONTRACTS, kind)) {
    throw new Error(`[typed_context.unsupported_kind] Unsupported typed context kind: ${kind || '<empty>'}`)
  }
  const contract = KIND_CONTRACTS[kind]
  const authority = String(input.authority ?? contract.authority).trim()
  const trust = String(input.trust ?? contract.trust).trim()
  if (authority !== contract.authority || trust !== contract.trust) {
    throw new Error(`[typed_context.authority_mismatch] ${kind} requires authority=${contract.authority}, trust=${contract.trust}.`)
  }
  const version = positiveInteger(input.version ?? 1, 'fragment_version')
  if (version !== 1) throw new Error(`[typed_context.unsupported_fragment_version] Unsupported fragment version: ${version}`)
  const lifecycle = String(input.lifecycle ?? 'turn').trim()
  if (!LIFECYCLES.has(lifecycle)) {
    throw new Error(`[typed_context.unsupported_lifecycle] Unsupported context lifecycle: ${lifecycle || '<empty>'}`)
  }
  const budget = normalizeBudget(input.budget)
  const content = neutralizeTypedContextMarkers(input.content ?? '')
  if (content.length > budget.maxChars) {
    throw new Error(`[typed_context.budget_exceeded] ${id} content exceeds maxChars=${budget.maxChars}.`)
  }
  const fragment = {
    schemaVersion,
    id,
    kind,
    authority,
    trust,
    version,
    budget,
    lifecycle,
    content,
  }
  return deepFreeze({ ...fragment, hash: typedContextHash(hashInput(fragment)) })
}

export function validateTypedContextFragment(fragment) {
  exactKeys(fragment, ROOT_KEYS, 'fragment')
  if (fragment.schemaVersion !== TYPED_CONTEXT_SCHEMA_VERSION) {
    throw new Error(`[typed_context.unsupported_schema] Unsupported typed context schema version: ${String(fragment.schemaVersion)}`)
  }
  const normalized = createTypedContextFragment({
    schemaVersion: fragment.schemaVersion,
    id: fragment.id,
    kind: fragment.kind,
    authority: fragment.authority,
    trust: fragment.trust,
    version: fragment.version,
    budget: fragment.budget,
    lifecycle: fragment.lifecycle,
    content: fragment.content,
  })
  if (typeof fragment.hash !== 'string' || !HASH_PATTERN.test(fragment.hash) || fragment.hash !== normalized.hash) {
    throw new Error(`[typed_context.hash_mismatch] Typed context fragment ${normalized.id} was modified after hashing.`)
  }
  return normalized
}

export function validateTypedContextSet(fragments, { requireWorkSnapshot = true } = {}) {
  if (!Array.isArray(fragments)) {
    throw new Error('[typed_context.invalid_set] Typed context must be an array of fragment objects.')
  }
  const validated = fragments.map((fragment) => validateTypedContextFragment(fragment))
  const ids = new Set()
  for (const fragment of validated) {
    if (ids.has(fragment.id)) throw new Error(`[typed_context.duplicate_id] Duplicate context fragment id: ${fragment.id}`)
    ids.add(fragment.id)
  }
  const workSnapshots = validated.filter((fragment) => fragment.kind === 'work_snapshot')
  if (requireWorkSnapshot && workSnapshots.length !== 1) {
    throw new Error(`[typed_context.work_snapshot_count] Expected exactly one Host WorkSnapshot fragment, received ${workSnapshots.length}.`)
  }
  if (!requireWorkSnapshot && workSnapshots.length > 1) {
    throw new Error(`[typed_context.work_snapshot_count] Expected at most one Host WorkSnapshot fragment, received ${workSnapshots.length}.`)
  }
  return Object.freeze(validated)
}

export function renderTypedContextFragment(fragment, contentOverride) {
  const validated = validateTypedContextFragment(fragment)
  const content = neutralizeTypedContextMarkers(contentOverride === undefined ? validated.content : contentOverride)
  if (content.length > validated.budget.maxChars) {
    throw new Error(`[typed_context.budget_exceeded] ${validated.id} rendered content exceeds maxChars=${validated.budget.maxChars}.`)
  }
  return '<fox_context_block kind="' + validated.kind + '" authority="' + validated.authority + '">\n'
    + content
    + '\n</fox_context_block>'
}

export function typedContextFragmentSnapshot(fragment) {
  const validated = validateTypedContextFragment(fragment)
  return deepFreeze({
    schemaVersion: validated.schemaVersion,
    id: validated.id,
    kind: validated.kind,
    authority: validated.authority,
    trust: validated.trust,
    version: validated.version,
    hash: validated.hash,
    budget: { ...validated.budget },
    lifecycle: validated.lifecycle,
    contentChars: validated.content.length,
  })
}
