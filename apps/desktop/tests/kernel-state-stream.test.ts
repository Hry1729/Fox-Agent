import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { createCoalescedRefresh } from '../src/features/conversations/model/coalesced-refresh'

// Match the existing hook-action harness: execute production subscription code
// with deterministic effects/timers, without a native WebView or model account.
const source = readFileSync(new URL('../src/features/conversations/hooks/use-kernel-state-stream.ts', import.meta.url), 'utf8')
const body = new Bun.Transpiler({ loader: 'ts' }).transformSync(source.slice(source.indexOf('export function')).replace('export function', 'function'))

function harness({ active = false }: { active?: boolean } = {}) {
  let cleanup!: () => void
  const timers: Array<() => void> = []
  const intervals: Array<() => void> = []
  let loads = 0
  let merged = 0
  let state: any = { conversation: { id: 'conversation-1' } }
  const env = {
    useEffect: (effect: () => () => void) => { cleanup = effect() },
    desktopRuntimeAvailable: true,
    desktopClient: {
      listenKernelModelPreviews: async () => () => {},
      loadConversation: async () => { loads++; return { conversation: { id: 'conversation-1' } } },
    },
    createCoalescedRefresh: (options: any) => createCoalescedRefresh({ ...options, schedule: (callback) => { let cancelled = false; timers.push(() => { if (!cancelled) callback() }); return () => { cancelled = true } } }),
    mergeConversationDetail: (next: any) => { merged++; return next },
  }
  const window = { setInterval: (callback: () => void) => { intervals.push(callback); return intervals.length }, clearInterval: () => { intervals.length = 0 } }
  const hook = new Function('window', ...Object.keys(env), `${body}; return useKernelStateStream`)(window, ...Object.values(env))
  hook({ conversationId: 'conversation-1', active, setDetail: (update: any) => { state = update(state) }, setError: () => {}, setErrorDetails: () => {} })
  return {
    fire: async () => { timers.shift()?.(); await Promise.resolve(); await Promise.resolve() },
    tick: () => intervals.forEach((callback) => callback()),
    switchConversation: () => { state = { conversation: { id: 'conversation-2' } } },
    cleanup: () => cleanup(), counts: () => ({ loads, merged, intervals: intervals.length }),
  }
}

describe('Kernel desktop subscription', () => {
  test('reads persisted state once on open and polls only while the run is active', async () => {
    const idle = harness()
    await idle.fire()
    expect(idle.counts()).toEqual({ loads: 1, merged: 1, intervals: 0 })
    idle.cleanup()
    const h = harness({ active: true })
    await h.fire()
    expect(h.counts().loads).toBe(1)
    for (let i = 0; i < 100; i++) h.tick()
    await h.fire()
    expect(h.counts().loads).toBe(2)
    h.cleanup()
    expect(h.counts().intervals).toBe(0)
  })
  test('never updates a different selected conversation and stops after cleanup', async () => {
    const cancelled = harness()
    cancelled.cleanup(); await cancelled.fire()
    expect(cancelled.counts()).toEqual({ loads: 0, merged: 0, intervals: 0 })
    const h = harness()
    h.switchConversation(); await h.fire()
    expect(h.counts().merged).toBe(0)
    h.cleanup()
  })
})
