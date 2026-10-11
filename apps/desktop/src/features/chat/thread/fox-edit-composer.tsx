import { ComposerPrimitive, useAui, useAuiState } from '@assistant-ui/react'
import { EditMessage } from '@/components/assistant-ui/elements/edit-message'

/**
 * The edit surface for the last user message (migration step 5C). The composer
 * primitives bind to the message's own edit composer: 取消 closes it, 发送
 * routes through the runtime's `onEdit` → `rerunFromMessage` (which truncates
 * later messages).
 */
export function FoxEditComposer() {
  const aui = useAui()
  const isEditing = useAuiState((state) => state.message.composer.isEditing)
  const value = useAuiState(state => state.message.composer.text)
  const discardedReplies = useAuiState(state => {
    const index = state.thread.messages.findIndex(message => message.id === state.message.id)
    return state.thread.messages.slice(index + 1).filter(message => message.role === 'assistant').length
  })
  if (!isEditing) return null
  return (
    <ComposerPrimitive.Root className="fox-edit-composer">
      <EditMessage value={value} onValueChange={text => aui.composer.setText(text)} onSave={() => aui.composer.send()} onCancel={() => aui.composer.cancel()} saving={false} discardedReplies={discardedReplies} />
    </ComposerPrimitive.Root>
  )
}
