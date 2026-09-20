/**
 * Security fixture factory for harness-security tests (role F).
 *
 * Everything is created INSIDE the isolated per-run tmp directory configured by
 * Enter-Role.ps1. No user data, no real secrets, no real credentials are read.
 * The "secrets" below are synthetic canaries that exist only to prove whether a
 * boundary would stop an untrusted path from reaching them.
 *
 * Layout created by `createProjectWithCanaries()`:
 *
 *   <root>/
 *     inside.txt               "public-data"          (inside the project)
 *     secret-canary.txt        "FOX-CANARY-SECRET"    (inside the project)
 *     link-to-outside.txt      -> outside/leaked.txt  (symlink escape vector)
 *     deep/
 *       note-<n>.txt           many-match corpus      (B02 continuation corpus)
 *     outside-canary/                                 (OUTSIDE the project root)
 *       leaked.txt             "OUTSIDE-CANARY-DATA"  (must not be reachable)
 *
 * `createOutsideTarget()` returns the sibling directory that the project must
 * NOT be able to read through a path traversal, absolute path, or symlink.
 */
import { mkdir, mkdtemp, rm, symlink, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'

/**
 * Whether this environment permits creating the reparse points (symlink or
 * directory junction) that the S01 escape cases need. Windows requires a
 * privilege for symbolic links, and the sandboxed harness blocks both forms
 * with EPERM; callers must degrade those cases to `not_run` with this reason
 * instead of silently dropping the negative case.
 */
export let linkCreationBlocked = null
export const LINK_BLOCKED_REASON = 'symlink/junction creation not permitted in this environment (EPERM)'

export const CANARY_SECRET = 'FOX-CANARY-SECRET'
export const OUTSIDE_CANARY = 'OUTSIDE-CANARY-DATA'
export const MATCH_LINE = 'needle-in-haystack'
export const MATCH_COUNT = 250

/**
 * A project root plus the outside-of-project canary directory it must not reach.
 * Both live under the isolated tmp root; `outside` is a sibling, never a child,
 * so escaping the project root is required to read it.
 */
export async function createProjectWithCanaries() {
  const base = await mkdtemp(join(tmpdir(), 'fox-harness-security-'))
  const root = join(base, 'project')
  const outside = join(base, 'outside-canary')
  await mkdir(root, { recursive: true })
  await mkdir(outside, { recursive: true })

  await writeFile(join(root, 'inside.txt'), 'public-data', 'utf8')
  await writeFile(join(root, 'secret-canary.txt'), CANARY_SECRET, 'utf8')
  await writeFile(join(outside, 'leaked.txt'), OUTSIDE_CANARY, 'utf8')

  // Symlink inside the project that points outside it. Both symbolic links and
  // directory junctions need a Windows privilege; record why the case is
  // skipped rather than pretending the boundary was tested.
  try {
    await symlink(join(outside, 'leaked.txt'), join(root, 'link-to-outside.txt'), 'file')
    linkCreationBlocked = null
  } catch (error) {
    linkCreationBlocked = error.code || error.message
  }

  // Corpus with more matches than maxMatches (200) so the continuation boundary
  // is exercised rather than inferred.
  const deep = join(root, 'deep')
  await mkdir(deep)
  const lines = Array.from({ length: MATCH_COUNT }, (_, index) => `${MATCH_LINE}-${index}`)
  await writeFile(join(deep, 'corpus.txt'), lines.join('\n'), 'utf8')

  return { base, root, outside, cleanup: () => rm(base, { recursive: true, force: true }) }
}

/** A file outside the project, addressed by absolute path (traversal vector). */
export function outsideAbsolute(outside) {
  return join(outside, 'leaked.txt')
}

/** Path that escapes the project root with a `..` traversal. */
export function traversalFromRoot(root) {
  return join(root, '..', 'outside-canary', 'leaked.txt')
}
