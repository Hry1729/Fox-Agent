/** Invalidations are hints, never state. Bound both concurrent reads and retries. */
export function createCoalescedRefresh<T>({
  load, apply, fail,
  schedule = (callback: () => void, delay: number) => {
    const timer = setTimeout(callback, delay)
    return () => clearTimeout(timer)
  },
}: {
  load: () => Promise<T>
  apply: (value: T) => void
  fail: (error: unknown) => void
  schedule?: (callback: () => void, delay: number) => () => void
}) {
  let disposed = false
  let running = false
  let dirty = false
  let failures = 0
  let cancelPending: (() => void) | null = null
  const queue = () => {
    if (disposed || running || cancelPending || !dirty) return
    cancelPending = schedule(() => { cancelPending = null; void refresh() }, failures ? 250 * failures : 100)
  }
  const refresh = async () => {
    if (disposed) return
    dirty = false
    running = true
    try {
      const value = await load()
      if (!disposed) apply(value)
      failures = 0
    } catch (error) {
      if (!disposed) {
        failures += 1
        if (failures < 3) dirty = true
        else {
          dirty = false
          failures = 0
          fail(error)
        }
      }
    } finally {
      running = false
      queue()
    }
  }
  return {
    invalidate() { if (!disposed) { dirty = true; queue() } },
    dispose() { disposed = true; dirty = false; cancelPending?.(); cancelPending = null },
  }
}
