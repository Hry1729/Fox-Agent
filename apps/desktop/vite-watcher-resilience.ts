import type { Plugin, ViteDevServer } from 'vite'
import { normalizePath } from 'vite'
import { existsSync } from 'node:fs'

/**
 * Bounded resilience for the dev-server file watcher.
 *
 * What this is *not*: it is not the reason the project folder no longer kills
 * the dev server, and it deliberately does not swallow anything globally.
 * Nothing here touches `process.on('uncaughtException')` or
 * `unhandledRejection`, no code path disables watching, and an error this
 * handler cannot classify is reported and otherwise left alone.
 *
 * Why it exists at all: on Windows a watcher can legitimately hit a sharing
 * violation (`EBUSY` / `EPERM`) on a path that is *transiently* held open —
 * antivirus, the indexer, or any tool that briefly opens a file for writing.
 * Those are recoverable: the path is still wanted, and re-arming it after the
 * holder lets go is enough. A watcher that silently stops watching such a path
 * produces the worst failure mode there is, a process that looks alive while
 * HMR is dead.
 *
 * The measurement behind the narrow scope (see the R4 evidence in
 * `output/artifact-placement-repair-20260918/`): Vite 6.4.3's bundled watcher
 * does **not** exit when a file inside the watched tree is held with
 * `FileShare.None`, with or without these ignore rules, with or without the
 * React/Tailwind plugins. The real fix for the original crash was moving the
 * Office working copy and its commit staging out of the project folder; this
 * handler only closes the remaining "watcher quietly stops" gap.
 *
 * ## Bounds (this is the part that used to be missing)
 *
 * The first version of this handler armed one fresh timer per error with no
 * dedup, no ceiling and no teardown, so a persistent failure accumulated timers
 * without limit: 40 injected errors produced 40 timers and 140 log lines, and
 * 100 re-arm runs left the 40 timers still pending. Recovery is now:
 *
 * * **deduplicated per path** — one recovery state per path, so an error storm
 *   on one path cannot create more than one pending timer;
 * * **bounded** — finite exponential backoff, a finite number of re-arm
 *   attempts per path, and a finite number of paths pending at once;
 * * **verified** — an `add()` is never treated as success because it did not
 *   throw; the next tick checks the watcher's own `getWatched()` to see whether
 *   the path is really watched again, and only then is the state cleared;
 * * **torn down** — timers and this plugin's own listener are removed when the
 *   dev server closes, and the plugin never registers twice on one watcher.
 *
 * These limits govern watcher fault recovery only. They are unrelated to, and
 * must never be confused with, any Fox task budget: no task duration or round
 * limit is introduced here.
 */

/** Recovery limits. Exported so tests assert against one source of truth. */
export const WATCHER_RECOVERY_LIMITS = Object.freeze({
  /** One pending recovery state per distinct path, at most this many at once. */
  maxPendingPaths: 32,
  /** `watcher.add` calls per path before the handler gives up on that path. */
  maxAttemptsPerPath: 5,
  /** First retry delay; doubled per attempt up to `maxDelayMs`. */
  baseDelayMs: 250,
  maxDelayMs: 4_000,
  /** At most one budget/give-up notice per interval, per dev server. */
  diagnosticIntervalMs: 30_000,
})

/** The repository's own transient names: never worth re-watching. */
const HOST_TRANSIENT_NAME = /\.fox-office-|\.fox-backup-|\.fox-stage-|\.work-.*\.batch/

/** Glob metacharacters. A string entry containing one cannot be evaluated here. */
const GLOB_META = /[*?[\]{}()!+@]/

/**
 * Normalize a path for comparison.
 *
 * The Windows verbatim prefix is removed **before** Vite's `normalizePath` runs:
 * that helper is `path.posix.normalize`, which collapses the prefix's leading
 * double slash (`\\?\C:\x` becomes `/C:/x`) and would destroy the very thing we
 * need to strip. A verbatim UNC prefix (`\\?\UNC\server\share`) is folded back
 * into the ordinary UNC form first. Any trailing separator is dropped, and the
 * result is folded on Windows where paths are case-insensitive.
 */
function toComparable(value: string): string {
  let raw = value
  if (raw.startsWith('\\\\?\\UNC\\') || raw.startsWith('//?/UNC/')) {
    raw = `\\\\${raw.slice(8)}`
  } else if (
    raw.startsWith('\\\\?\\') ||
    raw.startsWith('\\\\.\\') ||
    raw.startsWith('//?/') ||
    raw.startsWith('//./')
  ) {
    raw = raw.slice(4)
  }
  const text = normalizePath(raw).replace(/\/+$/, '')
  return process.platform === 'win32' ? text.toLowerCase() : text
}

/**
 * Whether `path` is the root itself or lives inside it.
 *
 * A plain `startsWith(root)` is not a directory-boundary check: with root
 * `D:/x/app` it accepts `D:/x/app-other/...`, which is a different project and
 * may not be re-armed.
 */
export function isInsideRoot(root: string, path: string): boolean {
  const normalizedRoot = toComparable(root)
  if (normalizedRoot.length === 0) return false
  const candidate = toComparable(path)
  return candidate === normalizedRoot || candidate.startsWith(`${normalizedRoot}/`)
}

/**
 * Whether the configured isolation rules exclude a path.
 *
 * * `ignored`  — an entry matches; the path must not be re-armed.
 * * `included` — no entry matches; re-arming is allowed.
 * * `unknown`  — an entry could not be evaluated. Re-arming is *not* allowed.
 *
 * RegExp entries are evaluated exactly (they are what this repository
 * configures). String entries are only honoured when they hold no glob
 * metacharacter, in which case they are compared literally on a directory
 * boundary. A recursive-glob string (the double-star kind, e.g. the usual
 * node_modules pattern) is deliberately reported as `unknown` rather than
 * approximated: a substring test pretending to be glob matching would silently
 * judge such a pattern as "not ignored" and re-arm a path the user meant to
 * exclude.
 */
export function watcherIgnoreVerdict(
  path: string,
  configured: unknown,
): 'ignored' | 'included' | 'unknown' {
  if (HOST_TRANSIENT_NAME.test(path)) return 'ignored'
  const list = Array.isArray(configured) ? configured : configured ? [configured] : []
  const candidate = toComparable(path)
  for (const entry of list) {
    if (entry instanceof RegExp) {
      // A `g`/`y` RegExp carries `lastIndex` between calls; reset it so the
      // verdict does not depend on how many times this ran before.
      entry.lastIndex = 0
      if (entry.test(path)) return 'ignored'
      continue
    }
    if (typeof entry !== 'string') return 'unknown'
    if (GLOB_META.test(entry)) return 'unknown'
    const literal = toComparable(entry)
    if (literal.length === 0) continue
    if (candidate === literal || candidate.startsWith(`${literal}/`)) return 'ignored'
  }
  return 'included'
}

/** Whether the watcher reports this path as watched right now. `null` = cannot tell. */
function watchedState(watcher: ViteDevServer['watcher'], path: string): boolean | null {
  const getWatched = (watcher as { getWatched?: () => Record<string, string[]> }).getWatched
  if (typeof getWatched !== 'function') return null
  let watched: unknown
  try {
    watched = getWatched.call(watcher)
  } catch {
    return null
  }
  // A watcher that answers with anything but a directory map cannot tell us
  // whether the path is watched; that is "unverified", never "not watched".
  if (watched === null || typeof watched !== 'object') return null
  const candidate = toComparable(path)
  for (const [directory, names] of Object.entries(watched as Record<string, string[]>)) {
    if (toComparable(directory) === candidate) return true
    for (const name of names ?? []) {
      if (toComparable(`${directory}/${name}`) === candidate) return true
    }
  }
  return false
}

interface RecoveryState {
  attempts: number
  duplicates: number
  timer: ReturnType<typeof setTimeout> | null
}

export function watcherResilience(): Plugin {
  // One registration per watcher: a config reload that re-applies the plugin
  // must not stack a second `error` listener on the same emitter.
  const configured = new WeakSet<object>()

  return {
    name: 'fox:watcher-resilience',
    apply: 'serve',
    configureServer(server: ViteDevServer) {
      const watcher = server.watcher
      if (configured.has(watcher)) return
      configured.add(watcher)

      const logger = server.config.logger
      const root = server.config.root
      const ignored = server.config.server.watch?.ignored
      const pending = new Map<string, RecoveryState>()
      let disposed = false
      // `0` means "nothing emitted yet", which makes the *first* budget notice
      // immediate. Throttling must never swallow the first diagnostic: a
      // give-up that nothing reports is indistinguishable from silence.
      let lastNotifiedAt = 0
      let suppressed = 0

      /** At most one notice per interval after the first; nothing is lost silently. */
      const notifyBudget = (message: string) => {
        suppressed += 1
        const now = Date.now()
        if (lastNotifiedAt !== 0 && now - lastNotifiedAt < WATCHER_RECOVERY_LIMITS.diagnosticIntervalMs) {
          return
        }
        const extra = suppressed > 1 ? ` (${suppressed} notices in this window)` : ''
        logger.warn(`${message}${extra}`)
        lastNotifiedAt = now
        suppressed = 0
      }

      const armAttempt = (path: string, state: RecoveryState) => {
        const delay = Math.min(
          WATCHER_RECOVERY_LIMITS.baseDelayMs * 2 ** state.attempts,
          WATCHER_RECOVERY_LIMITS.maxDelayMs,
        )
        state.timer = setTimeout(() => {
          state.timer = null
          if (disposed || watcher.closed) {
            pending.delete(path)
            return
          }

          // Verify the *previous* attempt before making another one. A settle
          // period has passed, so `getWatched()` can answer truthfully.
          if (state.attempts > 0) {
            const watched = watchedState(watcher, path)
            if (watched === true) {
              const coalesced = state.duplicates > 1 ? ` (+${state.duplicates - 1} coalesced error(s))` : ''
              pending.delete(path)
              logger.info(`[fox] watcher recovery: re-armed ${path} after ${state.attempts} attempt(s)${coalesced}`)
              return
            }
            if (watched === null) {
              // The watcher exposes no read-back, so recovery cannot be proven.
              // Stop rather than loop, and say exactly that.
              pending.delete(path)
              logger.warn(
                `[fox] watcher recovery: issued add() for ${path} but this watcher exposes no getWatched(); recovery of that path is unverified`,
              )
              return
            }
            if (state.attempts >= WATCHER_RECOVERY_LIMITS.maxAttemptsPerPath) {
              pending.delete(path)
              notifyBudget(
                `[fox] watcher recovery gave up on ${path} after ${state.attempts} attempt(s); the rest of the watcher is unaffected and still watching`,
              )
              return
            }
          }

          if (!existsSync(path)) {
            // Nothing to re-arm; that is a resolved state, not a failure.
            pending.delete(path)
            logger.info(`[fox] watcher recovery: ${path} no longer exists; nothing to re-arm`)
            return
          }

          state.attempts += 1
          try {
            watcher.add(path)
          } catch (retryError) {
            logger.warn(
              `[fox] watcher recovery: add() for ${path} threw: ${
                (retryError as Error)?.message ?? String(retryError)
              }`,
            )
          }
          armAttempt(path, state)
        }, delay)
      }

      const schedule = (path: string) => {
        const existing = pending.get(path)
        if (existing) {
          // Coalesced: one recovery state per path, so an error storm cannot
          // create more timers than there are paths.
          existing.duplicates += 1
          notifyBudget(`[fox] watcher recovery: repeated errors on ${path}`)
          return
        }
        if (pending.size >= WATCHER_RECOVERY_LIMITS.maxPendingPaths) {
          notifyBudget(
            `[fox] watcher recovery: pending-path budget (${WATCHER_RECOVERY_LIMITS.maxPendingPaths}) reached; not arming another path`,
          )
          return
        }
        const state: RecoveryState = { attempts: 0, duplicates: 1, timer: null }
        pending.set(path, state)
        logger.warn(
          `[fox] watcher recovery armed for ${path}; re-arming it until the holder releases it`,
        )
        armAttempt(path, state)
      }

      const onError = (error: unknown) => {
        const failure = error as NodeJS.ErrnoException & { path?: string }
        const code = failure?.code
        const path = typeof failure?.path === 'string' ? failure.path : ''
        const recoverableClass =
          (code === 'EBUSY' || code === 'EPERM') && path.length > 0 && isInsideRoot(root, path)

        if (!recoverableClass) {
          // Unknown, out of scope, or an explicitly ignored internal path: say
          // so plainly. The watcher keeps its own state and is never disabled.
          logger.error(
            `[fox] watcher error${code ? ` ${code}` : ''}${path ? ` on ${path}` : ''}: ${
              failure?.message ?? String(error)
            }`,
          )
          return
        }

        const verdict = watcherIgnoreVerdict(path, ignored)
        if (verdict !== 'included') {
          logger.error(
            `[fox] watcher error ${code} on ${path} was not re-armed (${verdict === 'ignored' ? 'path is isolated' : 'isolation rules could not be evaluated'})`,
          )
          return
        }

        schedule(path)
      }

      watcher.on('error', onError)

      // Teardown: clear every timer and remove this plugin's own listener, so a
      // closed or restarted dev server cannot accumulate state or handlers.
      const dispose = () => {
        if (disposed) return
        disposed = true
        for (const state of pending.values()) {
          if (state.timer) clearTimeout(state.timer)
        }
        pending.clear()
        watcher.off('error', onError)
        configured.delete(watcher)
      }

      server.httpServer?.once('close', dispose)
      // chokidar's FSWatcher does not always emit `close`; guard defensively.
      ;(watcher as unknown as { once?: (event: string, listener: () => void) => void }).once?.(
        'close',
        dispose,
      )
    },
  }
}
