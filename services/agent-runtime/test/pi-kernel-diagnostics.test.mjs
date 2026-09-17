import test from 'node:test'
import assert from 'node:assert/strict'
import { describeKernelError, diagnosticLine, redactDiagnosticText } from '../src/pi-kernel-diagnostics.mjs'

const secretish = 'sk-liveABCDEFGHIJKLMNOPQRSTUVWXYZ012345'

test('a worker diagnostic publishes only whitelisted fields, never a raw stack', () => {
  const error = new Error(`provider rejected the payload: ${'x'.repeat(5_000)}`)
  error.name = 'ProviderRequestError'
  error.status = 429
  error.cause = Object.assign(new Error('socket hang up'), { code: 'ECONNRESET' })
  const shape = describeKernelError(error)
  assert.deepEqual(
    Object.keys(shape).sort(),
    ['cause', 'code', 'frame', 'message', 'name', 'status'].sort(),
  )
  assert.equal(shape.name, 'ProviderRequestError')
  assert.equal(shape.status, 429)
  assert.equal(shape.code, 'ECONNRESET')
  assert.match(shape.frame, /^[\w.-]+\.mjs:\d+$/, shape.frame)
  assert.ok(
    !Object.values(shape).some(value => String(value).includes('\n')),
    'a stack has newlines; a diagnostic line must not',
  )
  assert.ok(shape.message.length <= 201, `message must be capped: ${shape.message?.length}`)
  // An unknown property of the thrown object cannot appear.
  const leaky = new Error('boom')
  leaky.requestBody = { messages: [{ role: 'user', content: 'private' }] }
  leaky.apiKey = secretish
  assert.equal(JSON.stringify(describeKernelError(leaky)).includes('private'), false)
  assert.equal(JSON.stringify(describeKernelError(leaky)).includes(secretish), false)
})

test('credentials and URL paths are removed from free text before publication', () => {
  const redacted = redactDiagnosticText(
    `POST https://api.example.com:8080/v1/chat/completions?key=${secretish} `
    + `Authorization: Bearer abc.def_123 password=hunter2`,
  )
  for (const secret of [secretish, 'hunter2', '/v1/chat/completions', 'abc.def_123']) {
    assert.ok(!redacted.includes(secret), `leaked ${secret} in: ${redacted}`)
  }
  assert.ok(redacted.includes('api.example.com'), 'the host alone stays for triage: ' + redacted)
  // A user:password authority never survives.
  assert.ok(
    !redactDiagnosticText('fetch https://user:pass@host/path failed').includes('pass'),
    redactDiagnosticText('fetch https://user:pass@host/path failed'),
  )
})

test('a diagnostic line is bounded no matter how long the fields are', () => {
  const huge = new Error('y'.repeat(200_000))
  huge.stack = `Error: ${'z'.repeat(200_000)}\n    at worker.mjs:1:1`
  const line = diagnosticLine('kernel-worker model round failed', describeKernelError(huge))
  assert.ok(line.length <= 900, `line length ${line.length}`)
  assert.ok(!/y{500}/.test(line), 'only a capped prefix of provider text survives')
  assert.ok(!/z{500}/.test(line), 'the stack never reaches the line at all')
})

test('the categorized evidence the Host already holds is echoed, other errors are inert', () => {
  const failure = new Error('Kernel model round failed; the Host owns retry admission')
  failure.evidence = { category: 'provider_unavailable', httpStatus: 503, retryAfterMs: 2000 }
  const shape = describeKernelError(failure)
  assert.equal(shape.evidenceCategory, 'provider_unavailable')
  assert.equal(shape.evidenceHttpStatus, 503)
  assert.equal(shape.retryAfterMs, undefined, 'only the fixed field list is published')
  assert.equal(describeKernelError(null), null)
  assert.equal(describeKernelError(undefined), null)
})

test('username-only URLs, truncated quoted secrets and machine tokens are redacted', () => {
  assert.equal(redactDiagnosticText('https://private-user@host/path'), 'https://host/')
  const error = new Error(`password="${'private-value'.repeat(100)}`)
  error.name = secretish
  error.code = secretish
  error.cause = { name: secretish }
  error.evidence = { category: secretish }
  const serialized = JSON.stringify(describeKernelError(error))
  assert.ok(!serialized.includes('private-value'))
  assert.ok(!serialized.includes(secretish))
  assert.equal(diagnosticLine('large', { value: 'x'.repeat(2000) }).length, 900)
})
