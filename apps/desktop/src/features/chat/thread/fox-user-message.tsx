import { useContacts } from '@/features/companions/context'
import { ContactQuoteRef, ContactUserMarks } from '@/features/companions/marks'
import { ContactTimeDivider } from '@/features/companions/chat-time'
import { useFoxThread } from './fox-thread-context'
import { ActionBarPrimitive, MessagePrimitive, useAui, useAuiState } from '@assistant-ui/react'
import type { TextMessagePartProps, Attachment as AuiAttachment } from '@assistant-ui/react'
import { UserMessageBubble } from '../components/UserMessageBubble'
import { requestConversationFork } from '../fox-shared'
import { FoxEditComposer } from './fox-edit-composer'
import { FileEdit, GitFork } from 'lucide-react'
import { File as MessageFile } from '@/components/assistant-ui/elements/file'
import { copyMessageResponse } from '@/components/assistant-ui/elements/message-actions'
import { FoxCopyMessageButton } from './fox-message-actions'
import { formatDateTime } from '@/lib/time-format'

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
  const companion = useContacts()
  const aui = useAui()
  const messageId = useAuiState((state) => state.message.id)
  const createdAt = useAuiState(state => state.message.createdAt)
  const isEditing = useAuiState((state) => state.message.composer.isEditing)
  const running = useAuiState(state => state.thread.isRunning)
  const isLatestUser = useAuiState(state => state.thread.messages.findLast(message => message.role === 'user')?.id === state.message.id)
  const contactChat = Boolean(companion?.privateChat)
  const fox = useFoxThread()
  // C07: a user bubble's durable identity is its message id. Until the bubble is
  // committed (an optimistic send, or an attachment-only message) it offers no
  // quote, favorite or reaction.
  const durableContact = contactChat && !isEditing && Boolean(fox.contactUserBubbleIds?.has(messageId))
  const quote = contactChat ? fox.contactMessageQuotes?.get(messageId) : undefined
  const contactMarks = durableContact && companion?.current && !isEditing
    ? <ContactUserMarks contact={companion.current} messageId={messageId} createdAt={createdAt}
        initialMark={fox.contactBubbleMarks?.get(messageId)}
        getText={() => aui.message.getState().content.flatMap(part => part.type === 'text' ? [part.text] : []).join('\n\n')} />
    : null
  const meta = !isEditing && <div className="fox-user-meta">
    {!contactChat && <time className="fox-message-time" dateTime={createdAt.toISOString()}>{formatDateTime(createdAt)}</time>}
    <ActionBarPrimitive.Root autohide="never" className="fox-user-actions">
      <FoxCopyMessageButton onCopy={() => copyMessageResponse(aui.message.getState().content.flatMap(part => part.type === 'text' ? [part.text] : []).join('\n\n'))} />
      {!companion?.privateChat && <button type="button" className="fox-thread-action-button" disabled={running} aria-label="从此消息创建分支" title="从此消息创建分支" onClick={() => requestConversationFork(aui.message.getState().id)}>
        <GitFork size={13} />
      </button>}
      {!companion?.privateChat && isLatestUser && <ActionBarPrimitive.Edit asChild disabled={running}>
          <button type="button" className="fox-thread-action-button" aria-label="编辑并重新发送" title="编辑并重新发送"><FileEdit size={13} /></button>
        </ActionBarPrimitive.Edit>}
    </ActionBarPrimitive.Root>
  </div>
  return (
    <div id={`fox-turn-${messageId}`} data-turn-message-id={messageId} className="fox-turn-anchor">
      {contactChat && <ContactTimeDivider messageId={messageId} />}
      <MessagePrimitive.Root className={`fox-thread-msg is-user${companion?.privateChat ? ' is-contact-user' : ''}`}>
        <MessagePrimitive.If hasAttachments>
          <div className="fox-thread-attachments">
            <MessagePrimitive.Attachments>
              {({ attachment }: { attachment: AuiAttachment }) => <FoxUserAttachment key={attachment.id} attachment={attachment} />}
            </MessagePrimitive.Attachments>
          </div>
        </MessagePrimitive.If>
        <FoxEditComposer />
        {contactChat
          ? <div className="fox-contact-user-turn">
              {quote && <ContactQuoteRef quote={quote} contactName={companion?.current?.displayName ?? '联系人'} />}
              <div className="fox-contact-user-row"><MessagePrimitive.Parts components={{ Text: FoxUserText }} />{contactMarks ?? (durableContact ? null : meta)}</div>
            </div>
          : <><MessagePrimitive.Parts components={{ Text: FoxUserText }} />{meta}</>}
      </MessagePrimitive.Root>
    </div>
  )
}
