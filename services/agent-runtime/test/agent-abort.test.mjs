import test from 'node:test'
import assert from 'node:assert/strict'
import { abortAgent } from '../src/agent-abort.mjs'

test('MD-4: a rejecting abort() never becomes an unhandledRejection and reaches onError', async () => {
  const unhandled = []
  const onUnhandled = reason => unhandled.push(reason)
  process.on('unhandledRejection', onUnhandled)
  try {
    const failures = []
    const rejecting = { abort: async () => { throw new Error('abort rejected') } }
    const throwing = { abort: () => { throw new Error('abort threw') } }
    abortAgent(rejecting, error => failures.push(error.message))
    abortAgent(throwing, error => failures.push(error.message))
    abortAgent(rejecting, () => { throw new Error('handler threw') })
    abortAgent(null)
    await new Promise(resolve => setTimeout(resolve, 20))
    assert.deepEqual(failures.sort(), ['abort rejected', 'abort threw'])
    assert.deepEqual(unhandled, [])
  } finally {
    process.off('unhandledRejection', onUnhandled)
  }
})

test('MD-4: a resolving abort() is called exactly once', async () => {
  let calls = 0
  abortAgent({ abort: async () => { calls += 1 } })
  await new Promise(resolve => setTimeout(resolve, 0))
  assert.equal(calls, 1)
})
