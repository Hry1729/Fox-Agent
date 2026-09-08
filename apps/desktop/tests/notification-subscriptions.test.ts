import { expect, test } from 'bun:test'
import { subscribeNotificationRefresh } from '../src/features/notification-subscriptions'

const flush = async () => { for (let i = 0; i < 5; i++) await Promise.resolve() }

function fixture(failed = '') {
  const handlers: Record<string, (event: any) => void> = {}
  const stopped: string[] = []
  const pending: Array<() => void> = []
  let refreshes = 0
  let failures = 0
  const names = ['listenRuntimeEvents', 'listenWorkEvents', 'listenApprovalRequests', 'listenApprovalResolved', 'listenKernelStateInvalidations']
  const client = Object.fromEntries(names.map((name) => [name, (handler: (event: any) => void) => {
    handlers[name] = handler
    return new Promise<() => void>((resolve, reject) => pending.push(() => name === failed
      ? reject(new Error('channel unavailable')) : resolve(() => stopped.push(name))))
  }]))
  const stop = subscribeNotificationRefresh(client as any, () => refreshes++, () => failures++)
  return { handlers, stopped, pending, stop, counts: () => ({ refreshes, failures }) }
}

test('Kernel commits refresh notifications while unsupported versions and text deltas do not', async () => {
  const h = fixture()
  await flush(); h.pending.forEach((resolve) => resolve()); await flush()
  expect(h.counts().refreshes).toBe(5)
  h.handlers.listenKernelStateInvalidations({ schemaVersion: 1 })
  h.handlers.listenKernelStateInvalidations({ schemaVersion: 2 })
  h.handlers.listenKernelStateInvalidations(null)
  h.handlers.listenRuntimeEvents({ event: { type: 'message.delta' } })
  expect(h.counts().refreshes).toBe(6)
  h.stop(); h.stop()
  expect(h.stopped.length).toBe(5)
  h.handlers.listenKernelStateInvalidations({ schemaVersion: 1 })
  expect(h.counts().refreshes).toBe(6)
})

test('failed and late channel registration still disposes every successful subscription', async () => {
  const h = fixture('listenWorkEvents')
  await flush()
  h.pending[0](); h.pending[1](); await flush()
  expect(h.counts()).toEqual({ refreshes: 1, failures: 1 })
  h.stop()
  h.pending.slice(2).forEach((resolve) => resolve()); await flush()
  expect(h.stopped.length).toBe(4)
  expect(h.counts()).toEqual({ refreshes: 1, failures: 1 })
})
