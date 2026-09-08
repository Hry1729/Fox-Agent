import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { createCoalescedRefresh } from '../src/features/conversations/model/coalesced-refresh'

// Match the existing hook-action harness: execute production subscription code
// with deterministic effects/timers, without a native WebView or model account.
const source = readFileSync(new URL('../src/features/conversations/hooks/use-kernel-state-stream.ts', import.meta.url), 'utf8')
const body = new Bun.Transpiler({ loader: 'ts' }).transformSync(source.slice(source.indexOf('export function')).replace('export function', 'function'))

function harness() {
  let cleanup!: () => void
  let listener!: (notice: unknown) => void
  let registered!: (stop: () => void) => void
  const timers: Array<() => void> = []
  let loads = 0
  let unlistens = 0
  let merged = 0
  let state: any = { conversation: { id: 'conversation-1' } }
  const env = {
    useEffect: (effect: () => () => void) => { cleanup = effect() },
    desktopRuntimeAvailable: true,
    desktopClient: {
      listenKernelModelPreviews: async () => () => {},
      listenKernelStateInvalidations: (handler: typeof listener) => { listener = handler; return new Promise<() => void>((resolve) => { registered = resolve }) },
      loadConversation: async () => { loads++; return { conversation: { id: 'conversation-1' } } },
    },
    createCoalescedRefresh: (options: any) => createCoalescedRefresh({ ...options, schedule: (callback) => { let cancelled = false; timers.push(() => { if (!cancelled) callback() }); return () => { cancelled = true } } }),
    mergeConversationDetail: (next: any) => { merged++; return next },
  }
  const hook = new Function(...Object.keys(env), `${body}; return useKernelStateStream`)(...Object.values(env))
  hook({ conversationId: 'conversation-1', setDetail: (update: any) => { state = update(state) }, setError: () => {}, setErrorDetails: () => {} })
  return {
    register: async () => { registered(() => { unlistens++ }); await Promise.resolve() },
    emit: (notice: unknown) => listener(notice),
    fire: async () => { timers.shift()?.(); await Promise.resolve(); await Promise.resolve() },
    switchConversation: () => { state = { conversation: { id: 'conversation-2' } } },
    cleanup: () => cleanup(), counts: () => ({ loads, unlistens, merged }),
  }
}

describe('Kernel desktop subscription', () => {
  test('refreshes after registration and accepts only supported invalidation versions', async () => {
    const h = harness()
    expect(h.counts().loads).toBe(0)
    await h.register(); await h.fire()
    expect(h.counts()).toEqual({ loads: 1, merged: 1, unlistens: 0 })
    h.emit(null); h.emit({ schemaVersion: 2 }); await h.fire()
    expect(h.counts().loads).toBe(1)
    for (let i = 0; i < 100; i++) h.emit({ schemaVersion: 1 })
    await h.fire()
    expect(h.counts().loads).toBe(2)
    h.cleanup()
    expect(h.counts().unlistens).toBe(1)
  })
  test('cleans up a late registration and never updates a different selected conversation', async () => {
    const cancelled = harness()
    cancelled.cleanup(); await cancelled.register(); await cancelled.fire()
    expect(cancelled.counts()).toEqual({ loads: 0, merged: 0, unlistens: 1 })
    const h = harness()
    await h.register()
    h.switchConversation(); await h.fire()
    expect(h.counts().merged).toBe(0)
    h.cleanup()
  })
})
