/**
 * The two sibling side cards of the workbench (SPEC item 4) and what the DOM owes
 * the native layer when growth is refused.
 *
 * Both cards cost the window real width, so their lifetime is a native-geometry
 * question and not just a render condition:
 *  - the contact card only makes sense inside the chat workspace — the management
 *    pages keep the SAME conversation selected, so an id that merely equals the
 *    selected conversation would keep the card (and its width declaration) mounted
 *    beside a settings page, and re-open it on the way back;
 *  - a refused growth has to roll back the card that ASKED for the missing room,
 *    which is not the last entry in a fixed render order: the controller reports
 *    the changed cards only (`SidebarGrowthRefusal`), each with the width the
 *    window actually reflects, so an already-open card that just widened is
 *    restored to that width instead of being closed, and a card the window never
 *    got is closed.
 */
import { useEffect, useRef } from 'react'
import type { SidebarGrowthRefusal } from '@/features/workspace/use-sidebar-window-growth'
import type { WorkspaceView } from '@/features/workspace/types'

export const RIGHT_PANEL_GROWTH_ID = 'right-panel'
export const CONTACT_CARD_GROWTH_ID = 'contact-card'

/** What the DOM has to undo after a refused growth pass, keyed by the card that
 *  asked for the room instead of by render order. */
export interface SideCardGrowthRollback {
  closeContactCard: boolean
  closeRightPanel: boolean
  /** The width the window really reflects for an already-open panel, or `null`
   *  when the panel was never accommodated and has to close. */
  restoreRightPanelWidth: number | null
}

/** Cards this workbench knows how to roll back are the only ones acted on: a new
 *  sibling card that has no DOM dismissal here must not dismiss someone else's. */
export function sideCardGrowthRollback(refusals: SidebarGrowthRefusal[]): SideCardGrowthRollback {
  let closeContactCard = false
  let closeRightPanel = false
  let restoreRightPanelWidth: number | null = null
  for (const refusal of refusals) {
    if (refusal.id === CONTACT_CARD_GROWTH_ID) closeContactCard = true
    else if (refusal.id === RIGHT_PANEL_GROWTH_ID) {
      if (refusal.accepted === null) closeRightPanel = true
      else restoreRightPanelWidth = refusal.accepted
    }
  }
  return { closeContactCard, closeRightPanel, restoreRightPanelWidth }
}

/** Chat-only gate for the sibling cards: `false` outside the chat workspace, and
 *  `onLeaveChat` runs on the way out so the open state is cleared instead of
 *  merely hidden (the predicate alone would reopen a stale card on return).
 *  `onLeaveChat` is read through a ref, so the clearing effect fires exactly on the
 *  chat / not-chat transition and never on a re-render of the same view. */
export function useChatWorkspaceSideCard(activeView: WorkspaceView, onLeaveChat: () => void): boolean {
  const insideChat = activeView === 'chat'
  const leaveRef = useRef(onLeaveChat)
  useEffect(() => { leaveRef.current = onLeaveChat })
  useEffect(() => { if (!insideChat) leaveRef.current() }, [insideChat])
  return insideChat
}
