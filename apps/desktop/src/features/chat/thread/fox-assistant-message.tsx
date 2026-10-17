import { ActionBarPrimitive, MessagePrimitive, useAui, useAuiState } from '@assistant-ui/react'
import type { DataMessagePartComponent } from '@assistant-ui/react'
import { TimelineRuntimeContext, requestConversationFork } from '../fox-shared'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { notify as toast } from '@/features/notifications'
import { useContext, useState } from 'react'
import { FoxArtifactsPart } from './parts/fox-artifacts'
import { FoxErrorPart } from './parts/fox-error'
import { FoxProcessPart } from './parts/fox-process'
import { FoxTextPart } from './parts/fox-text'
import { FoxRunActivityBody } from './fox-run-activity-body'
import type { FoxThreadMessage } from './convert-detail'
import { copyMessageResponse } from '@/components/assistant-ui/elements/message-actions'
import { FoxMessageActions } from './fox-message-actions'
import { FeedbackDialog as FeedbackDialogSurface } from '@/components/assistant-ui/elements/feedback-dialog'
import { StoppedRun } from '@/components/assistant-ui/elements/error-state'
import { ErrorPrompt } from '../demo/fox-demo-prompts'

const FOX_PART_COMPONENTS = {
  Text: FoxTextPart,
  data: {
    by_name: {
      'fox-process': FoxProcessPart,
      'fox-artifacts': FoxArtifactsPart,
      'fox-error': FoxErrorPart,
    } as Record<string, DataMessagePartComponent>,
  },
}

/** The negative-feedback dialog, carried over from the legacy message footer:
 * only category, comment and run identity are saved — never the answer body. */
function FoxFeedbackDialog({ open, onOpenChange, messageId, onSaved }: { open: boolean; onOpenChange: (open: boolean) => void; messageId: string; onSaved?: () => void }) {
  const [category, setCategory] = useState<'irrelevant' | 'code_error' | 'misunderstanding' | 'other'>('irrelevant')
  const [comment, setComment] = useState('')
  const [saving, setSaving] = useState(false)
  const submit = async () => {
    setSaving(true)
    try {
      await desktopClient.saveMessageFeedback(messageId, 'negative', category, comment.trim() || undefined)
      toast.success('反馈已保存')
      onOpenChange(false)
      setComment('')
      onSaved?.()
    } catch (cause) {
      toast.error('反馈保存失败', { description: desktopErrorDetails(cause).message })
    } finally {
      setSaving(false)
    }
  }
  return <FeedbackDialogSurface open={open} onOpenChange={onOpenChange} category={category} onCategoryChange={setCategory} note={comment} onNoteChange={setComment} saving={saving} onSubmit={() => void submit()} />
}

/**
 * The assistant message (migration step 5C): no frame, no background — the
 * answer itself is the content (720px cap). Parts map onto the Fox components:
 * process rows, Streamdown text, compact sources, artifacts and failures. The
 * Completed messages keep their footer visible; the active run reveals it at termination.
 */
export function FoxAssistantMessage() {
  const aui = useAui()
  const { assistantName } = useContext(TimelineRuntimeContext)
  const messageId = useAuiState((state) => state.message.id)
  const createdAt = useAuiState((state) => state.message.createdAt)
  const status = useAuiState((state) => state.message.status)
  const isLast = useAuiState(state => state.message.isLast)
  const foxMessage = useAuiState(state => state.message.metadata?.custom?.fox as FoxThreadMessage | undefined)
  const messageRunning = status?.type === 'running' || Boolean(foxMessage?.isActiveRun)
  const [feedbackOpen, setFeedbackOpen] = useState(false)
  const [reaction, setReaction] = useState<'positive' | 'negative' | null>(null)
  const [feedbackSaving, setFeedbackSaving] = useState(false)
  const submitPositive = async () => {
    setFeedbackSaving(true)
    try { await desktopClient.saveMessageFeedback(messageId, 'positive'); setReaction('positive'); toast.success('感谢反馈') }
    catch (cause) { toast.error('反馈保存失败', { description: desktopErrorDetails(cause).message }) }
    finally { setFeedbackSaving(false) }
  }
  const statusReason = status?.type === 'incomplete' ? status.reason : undefined
  const cancelled = statusReason === 'cancelled'
  const failed = statusReason === 'error'
  const replyTime = createdAt instanceof Date
    ? createdAt.toLocaleString([], { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
    : ''
  return (
    <div id={`fox-turn-${messageId}`} data-turn-message-id={messageId} className="fox-turn-anchor">
      <MessagePrimitive.Root className="fox-thread-msg is-assistant">
        {foxMessage?.source ? <FoxRunActivityBody message={foxMessage} /> : <MessagePrimitive.Parts components={FOX_PART_COMPONENTS} />}
        {foxMessage?.source && foxMessage.failure && <ErrorPrompt error={foxMessage.failure.message} onRetry={isLast ? () => aui.message.reload() : undefined} />}
        {!foxMessage?.source && cancelled && <StoppedRun reason="运行已取消，已生成内容保留" />}
        {!messageRunning && <div className="fox-message-footer">
          <ActionBarPrimitive.Root autohide="never" className="fox-thread-actionbar">
            <FoxMessageActions reaction={reaction} busy={feedbackSaving}
              onCopy={() => copyMessageResponse(aui.message.getState().content.flatMap(part => part.type === 'text' ? [part.text] : []).join('\n\n'))}
              onFork={() => requestConversationFork(aui.message.getState().id)} onPositive={() => void submitPositive()} onNegative={() => setFeedbackOpen(true)} onRegenerate={isLast ? () => aui.message.reload() : undefined} />
          </ActionBarPrimitive.Root>
          <div className="fox-message-meta">
            <span>{assistantName || 'Fox 默认助手'}</span>
            <span>{replyTime}</span>
            {cancelled && <span className="fox-message-cancelled-meta">已取消</span>}
            {failed && <span className="fox-message-failure-meta">运行失败</span>}
          </div>
        </div>}
      </MessagePrimitive.Root>
      <FoxFeedbackDialog open={feedbackOpen} onOpenChange={setFeedbackOpen} messageId={messageId} onSaved={() => setReaction('negative')} />
    </div>
  )
}

