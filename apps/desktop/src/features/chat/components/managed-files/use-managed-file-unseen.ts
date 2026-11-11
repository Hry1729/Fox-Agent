// Unseen-version marker ("orange dot") per surface and conversation.
// Rules: the first version set a user ever gets is the baseline and never dots
// (unchanged data must not nag); a stored fingerprint that differs from the
// loaded one dots; while the surface is actually showing its version list the
// current fingerprint is stored again and the dot clears.
import { useCallback, useEffect, useState } from 'react'

export type ManagedFileUnseenSurface = 'library' | 'files'

function seenKey(surface: ManagedFileUnseenSurface, conversationId: string) {
  return `fox:managed-file-seen:${surface}:${conversationId}`
}

function readSeen(surface: ManagedFileUnseenSurface, conversationId: string): string | null | undefined {
  try {
    const raw = globalThis.localStorage?.getItem(seenKey(surface, conversationId))
    return raw === undefined || raw === null ? null : raw
  } catch {
    return undefined // storage unavailable: never dot rather than nag
  }
}

function writeSeen(surface: ManagedFileUnseenSurface, conversationId: string, fingerprint: string) {
  try {
    globalThis.localStorage?.setItem(seenKey(surface, conversationId), fingerprint)
  } catch {
    // Ignore quota/privacy failures; the dot is a convenience, not state.
  }
}

export function useManagedFileUnseen(options: {
  surface: ManagedFileUnseenSurface
  conversationId: string | null | undefined
  /** Null while versions are still unknown (loading / failed / no thread). */
  fingerprint: string | null
  /** True exactly while this surface shows its version list to the user. */
  viewing: boolean
}): [unseen: boolean, markSeen: () => void] {
  const { surface, conversationId, fingerprint, viewing } = options
  const [unseen, setUnseen] = useState(false)

  useEffect(() => {
    setUnseen(false)
    if (!conversationId || fingerprint === null) return
    const stored = readSeen(surface, conversationId)
    if (stored === undefined) return
    if (stored === null) {
      writeSeen(surface, conversationId, fingerprint)
      return
    }
    setUnseen(stored !== fingerprint)
  }, [surface, conversationId, fingerprint])

  const markSeen = useCallback(() => {
    if (!conversationId || fingerprint === null) return
    writeSeen(surface, conversationId, fingerprint)
    setUnseen(false)
  }, [surface, conversationId, fingerprint])

  // Viewing the list IS the acknowledgement, including for fingerprints that
  // arrive while the list is open.
  useEffect(() => {
    if (viewing) markSeen()
  }, [viewing, markSeen])

  return [unseen, markSeen]
}
