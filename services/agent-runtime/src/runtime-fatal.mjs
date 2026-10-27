// Fatal JavaScript errors must never become an unexplained "closed stdout".
//
// An `uncaughtException` or an `unhandledRejection` used to end this process
// with nothing but a line on stderr. The Host only observes that the stdout
// pipe closed, so the Run was reported as `runtime.process_crashed` /
// "runtime process closed stdout" and the actual cause was lost with the
// child's stderr.
//
// The runtime now (1) ends every active Run with the same terminal frame a
// handled failure would produce, (2) keeps the stack on stderr for the Host's
// stderr tail, and (3) still exits non-zero, so a genuinely broken process is
// never mistaken for a finished one.
//
// This module is deliberately pure: the caller injects the active Runs, the
// terminal-frame writer, stderr and the exit path, so the behaviour is unit
// tested without killing the test process.

export const FATAL_CODES = Object.freeze({
  uncaughtException: 'runtime.uncaught_exception',
  unhandledRejection: 'runtime.unhandled_rejection',
})

/** Anything can be thrown or rejected; a terminal frame needs an Error. */
export function fatalError(reason) {
  if (reason instanceof Error) return reason
  if (typeof reason === 'string') return new Error(reason)
  try {
    return new Error(JSON.stringify(reason))
  } catch {
    return new Error(String(reason))
  }
}

function fatalMessage(kind, error) {
  const detail = error?.message || String(error)
  return kind === 'unhandledRejection'
    ? `Fox Pi Runtime aborted on an unhandled promise rejection: ${detail}`
    : `Fox Pi Runtime aborted on an uncaught exception: ${detail}`
}

/**
 * Build the process-level fatal handler.
 *
 * @param {object} options
 * @param {() => Array<object>} options.activeRuns     Runs that must become terminal.
 * @param {(run: object, code: string, message: string) => void} options.reportRunFailure
 * @param {(text: string) => void} [options.stderr]
 * @param {(code: number) => void} [options.exit]       Marks the exit status.
 * @param {() => void} [options.scheduleExit]           Flushes stdout, then exits.
 */
export function createFatalErrorHandler({
  activeRuns,
  reportRunFailure,
  stderr = (text) => process.stderr.write(text),
  exit = (code) => { process.exitCode = code },
  scheduleExit = () => { setTimeout(() => process.exit(1), 10) },
} = {}) {
  let handling = false
  return function handleFatal(reason, kind = 'uncaughtException') {
    const resolvedKind = kind === 'unhandledRejection' ? 'unhandledRejection' : 'uncaughtException'
    const code = FATAL_CODES[resolvedKind]
    const error = fatalError(reason)
    // A second fatal error while the first is being reported must not emit a
    // second terminal event or race the exit.
    if (handling) return { code, reported: 0, repeated: true }
    handling = true
    const stack = error?.stack || `${error?.name || 'Error'}: ${error?.message ?? String(reason)}`
    try {
      stderr(`[fox-pi-runtime] ${code}: ${stack}\n`)
    } catch {
      // stderr is the last diagnostic channel; it is never allowed to abort
      // the terminal report below.
    }
    let runs = []
    try {
      runs = activeRuns?.() ?? []
    } catch (failure) {
      try { stderr(`[fox-pi-runtime] ${code}: active Run lookup failed: ${failure?.message ?? failure}\n`) } catch {}
    }
    const message = fatalMessage(resolvedKind, error)
    let reported = 0
    for (const run of runs) {
      try {
        reportRunFailure?.(run, code, message)
        reported += 1
      } catch (failure) {
        try { stderr(`[fox-pi-runtime] ${code}: terminal Run report failed: ${failure?.message ?? failure}\n`) } catch {}
      }
    }
    try {
      exit(1)
    } finally {
      try { scheduleExit() } catch {}
    }
    return { code, reported, repeated: false }
  }
}
