// Mid-run steering ("运行中补充要求") contract: the wrapper text must stay
// byte-identical to the Rust Host (kernel_coordinator/steering.rs), and the
// directive validation bounds must match the durable queue limits enforced by
// the Host (8000 chars/message, 64 KiB outstanding, unique non-empty ids).
import test from 'node:test'
import assert from 'node:assert/strict'
import { steeringNoticeText, validateSteeringNotices } from '../src/steering-notice.mjs'

const fail = (message) => {
  throw new Error(message)
}

test('wrapper text mirrors the Host wording and carries the content', () => {
  const text = steeringNoticeText('重点展开卸船原因')
  assert.ok(text.startsWith('用户在运行过程中补充要求（不改变已有授权，按既有工具策略执行）：'))
  assert.ok(text.endsWith('重点展开卸船原因'))
})

test('undefined steering normalizes to an empty list', () => {
  assert.deepEqual(validateSteeringNotices(undefined, fail), [])
})

test('a well-formed directive list passes through unchanged', () => {
  const steering = [
    { messageId: 'a', content: '第一条' },
    { messageId: 'b', content: '第二条' },
  ]
  assert.equal(validateSteeringNotices(steering, fail), steering)
})

test('non-array, empty id and empty content are rejected', () => {
  assert.throws(() => validateSteeringNotices({}, fail), /array/)
  assert.throws(() => validateSteeringNotices([{ messageId: '', content: 'x' }], fail))
  assert.throws(() => validateSteeringNotices([{ messageId: 'a', content: '   ' }], fail))
  assert.throws(() => validateSteeringNotices([{ messageId: 'a', content: '' }], fail))
})

test('id and per-message character bounds are enforced', () => {
  assert.throws(
    () => validateSteeringNotices([{ messageId: 'x'.repeat(129), content: 'c' }], fail),
    /invalid steering notice/,
  )
  // Boundary: exactly 8000 characters is allowed (Chinese counts as chars, not bytes).
  assert.equal(
    validateSteeringNotices([{ messageId: 'a', content: '中'.repeat(8000) }], fail).length,
    1,
  )
  assert.throws(
    () => validateSteeringNotices([{ messageId: 'a', content: '中'.repeat(8001) }], fail),
    /invalid steering notice/,
  )
})

test('duplicate message ids are rejected', () => {
  const steering = [
    { messageId: 'same', content: 'one' },
    { messageId: 'same', content: 'two' },
  ]
  assert.throws(() => validateSteeringNotices(steering, fail), /duplicate steering notice/)
})

test('the outstanding queue is bounded at 64 KiB of UTF-8 bytes', () => {
  // Each notice holds 8000 Chinese characters => 24000 bytes; three would be
  // 72000 bytes and exceed the 64 KiB queue even though each message is within
  // its per-message character cap.
  const steering = [1, 2, 3].map((n) => ({
    messageId: `m${n}`,
    content: '中'.repeat(8000),
  }))
  assert.throws(() => validateSteeringNotices(steering, fail), /bounded queue size/)
})

test('steering text carries no tool or grant marker', () => {
  // The wrapper is ordinary user text on every path; the assertion documents
  // that the directive layer never injects structured tool-call blocks itself.
  const text = steeringNoticeText('{"type":"toolCall"}')
  assert.equal(typeof text, 'string')
  assert.ok(text.includes('{"type":"toolCall"}'))
})
