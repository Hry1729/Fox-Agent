import type { ConversationMessage } from '../../conversations/model/types'
import { useDesktopConversation } from '../../conversations/hooks/use-desktop-conversation'
import FoxThreadMinimal from './fox-thread-minimal'
import { useFoxRuntime, type FoxRuntimeCallbacks } from './use-fox-runtime'

type DesktopConversationState = ReturnType<typeof useDesktopConversation>

export interface AssistantUiChatProps {
  conversation: DesktopConversationState
  callbacks: FoxRuntimeCallbacks
  pendingMessage?: ConversationMessage | null
  isSendDisabled?: boolean
}

/**
 * The assistant-ui chat branch (migration step 5B). Mounted only while the
 * `fox.ui.chatEngine` switch is set to `assistant-ui`, and loaded as its own
 * lazy chunk so the legacy path and the first-screen budget stay untouched.
 */
export default function AssistantUiChat({ conversation, callbacks, pendingMessage, isSendDisabled }: AssistantUiChatProps) {
  const runtime = useFoxRuntime(conversation, callbacks, { pendingMessage, isSendDisabled })
  return <FoxThreadMinimal runtime={runtime} />
}
