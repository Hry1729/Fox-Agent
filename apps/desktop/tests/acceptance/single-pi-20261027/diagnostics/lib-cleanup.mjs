// Best-effort temp cleanup: Windows keeps a just-exited child's directory busy
// for a moment, and a diagnostics run must never fail because of cleanup.
import { rm } from 'node:fs/promises'

export async function safeRemove(dir, attempts = 6) {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      await rm(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
      return
    } catch {
      await new Promise(resolve => setTimeout(resolve, 150))
    }
  }
}
