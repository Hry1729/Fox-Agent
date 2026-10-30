// MD-4: Pi's AgentSession/Agent abort() is async. A rejection must never become
// an unhandledRejection, which `runtime-fatal.mjs` turns into a failure of every
// Run on this sidecar followed by process exit. The failure is reported on
// stderr and handed to `onError` (for example the Run's mapper) instead.
export function abortAgent(agent, onError = null, label = 'agent.abort') {
  if (!agent) return
  void Promise.resolve().then(() => agent.abort()).catch((error) => {
    process.stderr.write(`[fox-pi-runtime] ${label} failed: ${error instanceof Error ? error.message : String(error)}\n`)
    try { onError?.(error) } catch {}
  })
}
