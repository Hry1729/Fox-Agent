// The identity of one restore request, managed by the ACTION's lifecycle.
//
// A restore request is "undetermined" from the moment the user confirms it until
// the Host reports a definite end. While it is undetermined, a resend — a double
// click, a timeout retry — must carry the SAME identity so the Host can
// deduplicate instead of writing the file twice. Once the request has ended, the
// identity is released and the NEXT confirmation is a new action with a new
// identity, even for the very same version.

/** The action a confirmation belongs to: restoring one version, with or without force. */
export function restoreActionKey(versionId: string, force: boolean): string {
  return `${versionId}:${force ? 'force' : 'normal'}`
}

export class RestoreRequestIdentities {
  private readonly inFlight = new Map<string, string>()

  /**
   * The identity for one confirmation. An action that is still undetermined
   * keeps its identity; anything else mints a new one.
   */
  acquire(key: string, mint: () => string): string {
    const existing = this.inFlight.get(key)
    if (existing) return existing
    const created = mint()
    this.inFlight.set(key, created)
    return created
  }

  /** The request reached a definite end. The next confirmation is a new action. */
  release(key: string): void {
    this.inFlight.delete(key)
  }

  /** Whether this action is still undetermined (used by the UI to disable resends). */
  isPending(key: string): boolean {
    return this.inFlight.has(key)
  }

  get size(): number {
    return this.inFlight.size
  }
}
