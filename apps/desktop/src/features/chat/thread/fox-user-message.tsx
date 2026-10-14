import { ActionBarPrimitive, MessagePrimitive, useAui, useAuiState } from '@assistant-ui/react'
import type { TextMessagePartProps, Attachment as AuiAttachment } from '@assistant-ui/react'
import { UserMessageBubble } from '../components/UserMessageBubble'
import { requestConversationFork } from '../fox-shared'
import { FoxEditComposer } from './fox-edit-composer'
import { FileEdit, GitFork } from 'lucide-react'
import { File as MessageFile } from '@/components/assistant-ui/elements/file'
import { copyMessageResponse } from '@/components/assistant-ui/elements/message-actions'
import { FoxCopyMessageButton } from './fox-message-actions'

/** Raw user text — never markdown-rendered, same as the legacy bubble. */
function FoxUserText(part: TextMessagePartProps) {
  if (!part.text) return null
  return <UserMessageBubble>{part.text}</UserMessageBubble>
}

function FoxUserAttachment({ attachment }: { attachment: AuiAttachment }) {
  return (
    <MessageFile className="fox-thread-attachment" name={attachment.name} mediaType={attachment.contentType} />
  )
}

/**
 * The user message (migration step 5C): the only bubble with a background —
 * `--muted` with the large radius — plus its attachments and a visible
 * action row (复制 / 分叉 / 编辑). Only the last user message offers 编辑;
 * editing reruns from this message and truncates everything after it.
 */
export function FoxUserMessage() {
  const aui = useAui()
  const messageId = useAuiState((state) => state.message.id)
  const isEditing = useAuiState((state) => state.message.composer.isEditing)
  const running = useAuiState(state => state.thread.isRunning)
  const isLatestUser = useAuiState(state => state.thread.messages.findLast(message => message.role === 'user')?.id === state.message.id)
  return (
    <div id={`fox-turn-${messageId}`} data-turn-message-id={messageId} className="fox-turn-anchor">
      <MessagePrimitive.Root className="fox-thread-msg is-user">
        <MessagePrimitive.If hasAttachments>
          <div className="fox-thread-attachments">
            <MessagePrimitive.Attachments>
              {({ attachment }: { attachment: AuiAttachment }) => <FoxUserAttachment key={attachment.id} attachment={attachment} />}
            </MessagePrimitive.Attachments>
          </div>
        </MessagePrimitive.If>
        <MessagePrimitive.Parts components={{ Text: FoxUserText }} />
        <FoxEditComposer />
        {!isEditing && <div className="fox-user-meta">
            <ActionBarPrimitive.Root autohide="never" className="fox-user-actions">
              <FoxCopyMessageButton onCopy={() => copyMessageResponse(aui.message.getState().content.flatMap(part => part.type === 'text' ? [part.text] : []).join('\n\n'))} />
              <button type="button" className="fox-thread-action-button" disabled={running} aria-label="从此消息创建分支" title="从此消息创建分支" onClick={() => requestConversationFork(aui.message.getState().id)}>
                <GitFork size={13} />
              </button>
              {isLatestUser && <ActionBarPrimitive.Edit asChild disabled={running}>
                  <button type="button" className="fox-thread-action-button" aria-label="编辑并重新发送" title="编辑并重新发送"><FileEdit size={13} /></button>
                </ActionBarPrimitive.Edit>}
            </ActionBarPrimitive.Root>
          </div>}
      </MessagePrimitive.Root>
    </div>
  )
}
