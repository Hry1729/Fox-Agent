// Side-by-side comparison of what the two Fox Pi paths send to the provider.
//
// Runs every capture against a local recording stub (no credential, no network),
// compares the routing-relevant facts field by field and prints a single
// PASS/FAIL for:
//
//   "the SDK path and the Kernel path send the same base URL, path, model and protocol"
//
// It also writes `evidence/captured-requests.json` with the redacted requests and
// `evidence/comparison.md` with the side-by-side table.
//
// Usage: node compare-requests.mjs
import { mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { captureKernelPath, captureSdkPath } from './lib-capture.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const evidenceDir = join(here, 'evidence')

/** The facts that decide which route a provider is asked for. */
export function routingShape(record) {
  return {
    method: record.method,
    path: record.path,
    query: record.query,
    model: record.body?.model ?? null,
    stream: record.body?.stream ?? null,
    maxTokens: record.body?.max_tokens ?? record.body?.max_completion_tokens ?? record.body?.max_output_tokens ?? null,
    streamOptions: record.body?.stream_options ?? null,
    store: record.body?.store ?? null,
    bodyKeys: record.body ? Object.keys(record.body).sort().join(',') : '',
    authorizationScheme: (record.headers?.authorization ?? '').replace(/<redacted>$/, '').trim() || null,
  }
}

const ROUTING_FIELDS = ['method', 'path', 'query', 'model', 'stream', 'maxTokens', 'streamOptions', 'store', 'authorizationScheme']

export function compareShapes(shapes) {
  const differences = []
  const reference = shapes[0]
  for (const shape of shapes.slice(1)) {
    for (const field of ROUTING_FIELDS) {
      const left = JSON.stringify(reference.values[field])
      const right = JSON.stringify(shape.values[field])
      if (left !== right) differences.push({ field, [reference.label]: reference.values[field], [shape.label]: shape.values[field] })
    }
  }
  return differences
}

/** A bounded, redacted evidence record: routing facts, not the prompt itself. */
export function evidenceRecord(record) {
  return {
    phase: record.phase,
    method: record.method,
    url: record.url,
    path: record.path,
    httpVersion: record.httpVersion,
    headers: record.headers,
    bodyBytes: record.bodyBytes,
    routing: routingShape(record),
    messageRoles: Array.isArray(record.body?.messages) ? record.body.messages.map(message => message.role) : null,
    toolCount: Array.isArray(record.body?.tools) ? record.body.tools.length : null,
    bodyKeys: record.body ? Object.keys(record.body).sort() : null,
  }
}

function table(shapes) {
  const lines = []
  lines.push(`| field | ${shapes.map(shape => shape.label).join(' | ')} |`)
  lines.push(`| --- | ${shapes.map(() => '---').join(' | ')} |`)
  for (const field of ROUTING_FIELDS) {
    lines.push(`| ${field} | ${shapes.map(shape => JSON.stringify(shape.values[field])).join(' | ')} |`)
  }
  return lines.join('\n')
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const sdk = await captureSdkPath()
  const kernel = await captureKernelPath()

  const shapes = []
  for (const record of sdk.requests) shapes.push({ label: record.phase, record, values: routingShape(record) })
  for (const record of kernel.requests) shapes.push({ label: record.phase, record, values: routingShape(record) })

  const differences = compareShapes(shapes)
  const pass = shapes.length >= 3 && differences.length === 0

  mkdirSync(evidenceDir, { recursive: true })
  writeFileSync(join(evidenceDir, 'captured-requests.json'), JSON.stringify({
    capturedAt: new Date().toISOString(),
    note: 'authorization header values are replaced by <redacted>; no credential is used or stored',
    sdk: { baseUrl: sdk.baseUrl, runs: sdk.runs, requests: sdk.requests.map(evidenceRecord) },
    kernel: { baseUrl: kernel.baseUrl, outcome: kernel.outcome, requests: kernel.requests.map(evidenceRecord) },
    differences,
    pass,
  }, null, 2), 'utf8')

  writeFileSync(join(evidenceDir, 'comparison.md'), `# Captured provider requests (redacted)\n\n${table(shapes)}\n\n${differences.length ? `## Differences\n\n${JSON.stringify(differences, null, 2)}\n` : 'No routing-relevant difference: method, path, query, model, stream flag, token budget field/value, stream options, store flag and authorization scheme are identical.\n'}\n## Outcomes against the stub\n\n${[...sdk.runs, ...kernel.runs].map(run => `- ${run.label}: ${run.outcome}`).join('\n')}\n`, 'utf8')

  console.log(table(shapes))
  console.log('')
  for (const run of [...sdk.runs, ...kernel.runs]) console.log(`run ${run.label}: ${run.outcome}`)
  console.log('')
  if (differences.length) {
    console.log('differences:', JSON.stringify(differences, null, 2))
    console.log('FAIL  the two paths do not send the same routing-relevant request')
  } else {
    console.log(`PASS  SDK and Kernel paths send the same base URL, path, model and protocol (${ROUTING_FIELDS.join(', ')})`)
  }
  console.log(`evidence: ${join(evidenceDir, 'captured-requests.json')}`)
  process.exitCode = pass ? 0 : 1
}
