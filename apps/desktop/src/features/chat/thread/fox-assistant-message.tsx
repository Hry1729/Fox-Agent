import { ActionBarPrimitive, MessagePrimitive, useAui, useAuiState } from '@assistant-ui/react'
import type { DataMessagePartComponent } from '@assistant-ui/react'
import { FoxAssistantAvatar } from '../components/FoxAssistantAvatar'
import { TimelineRuntimeContext, copyTextWithFeedback, requestConversationFork } from '../fox-shared'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { notify as toast } from '@/features/notifications'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Textarea } from '@/components/ui/textarea'
import { Copy, GitFork, LoaderCircle, RotateCcw, ThumbsDown, ThumbsUp } from 'lucide-react'
import { useContext, useState } from 'react'
import { FoxArtifactsPart } from './parts/fox-artifacts'
import { FoxErrorPart } from './parts/fox-error'
import { FoxProcessPart } from './parts/fox-process'
import { FoxTextPart } from './parts/fox-text'

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
function FoxFeedbackDialog({ open, onOpenChange, messageId }: { open: boolean; onOpenChange: (open: boolean) => void; messageId: string }) {
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
    } catch (cause) {
      toast.error('反馈保存失败', { description: desktopErrorDetails(cause).message })
    } finally {
      setSaving(false)
    }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="fox-feedback-dialog">
        <DialogHeader><DialogTitle>这条回答哪里需要改进？</DialogTitle><DialogDescription>只保存分类、备注和运行标识，不保存回答正文、文件内容或推理过程。</DialogDescription></DialogHeader>
        <div className="fox-feedback-categories" role="radiogroup" aria-label="反馈分类">
          {([
            ['irrelevant', '答非所问'],
            ['code_error', '代码错误'],
            ['misunderstanding', '理解偏差'],
            ['other', '其他'],
          ] as const).map(([value, label]) => <button type="button" role="radio" aria-checked={category === value} className={category === value ? 'is-selected' : ''} key={value} onClick={() => setCategory(value)}>{label}</button>)}
        </div>
        <label className="fox-feedback-comment"><span>补充说明 <small>可选</small></span><Textarea value={comment} maxLength={1000} placeholder="告诉我们具体问题，最多 1000 字" onChange={(event) => setComment(event.target.value)} /><small>{comment.length} / 1000</small></label>
        <DialogFooter><Button variant="outline" onClick={() => onOpenChange(false)}>取消</Button><Button disabled={saving} onClick={() => void submit()}>{saving && <LoaderCircle className="animate-spin" />}提交反馈</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/**
 * The assistant message (migration step 5C): no frame, no background — the
 * answer itself is the content (720px cap). Parts map onto the Fox components:
 * process rows, Streamdown text, compact sources, artifacts and failures. The
 * action row is hover-revealed and stays visible on the last message
 * (`autohide="not-last"`); 反馈 uses the runtime's positive adapter and the
 * legacy category dialog for negative feedback.
 */
export function FoxAssistantMessage() {
  const aui = useAui()
  const { assistantName } = useContext(TimelineRuntimeContext)
  const messageId = useAuiState((state) => state.message.id)
  const createdAt = useAuiState((state) => state.message.createdAt)
  const status = useAuiState((state) => state.message.status)
  const [feedbackOpen, setFeedbackOpen] = useState(false)
  const statusReason = status?.type === 'incomplete' ? status.reason : undefined
  const cancelled = statusReason === 'cancelled'
  const failed = statusReason === 'error'
  const replyTime = createdAt instanceof Date
    ? createdAt.toLocaleString([], { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
    : ''
  return (
    <div id={`fox-turn-${messageId}`} data-turn-message-id={messageId} className="fox-turn-anchor">
      <MessagePrimitive.Root className="fox-thread-msg is-assistant">
        <div className="fox-thread-assistant-head"><FoxAssistantAvatar /></div>
        <MessagePrimitive.Parts components={FOX_PART_COMPONENTS} />
        <div className="fox-message-footer">
          <ActionBarPrimitive.Root hideWhenRunning autohide="not-last" className="fox-message-actions">
            <ActionBarPrimitive.Copy asChild>
              <button type="button" className="fox-thread-action-button" aria-label="复制回复"><Copy size={14} /></button>
            </ActionBarPrimitive.Copy>
            <button type="button" className="fox-thread-action-button" aria-label="从此消息创建分支" title="从此消息创建分支" onClick={() => requestConversationFork(aui.message.getState().id)}>
              <GitFork size={14} />
            </button>
            <ActionBarPrimitive.FeedbackPositive asChild>
              <button type="button" className="fox-thread-action-button" aria-label="这条回答有帮助"><ThumbsUp size={14} /></button>
            </ActionBarPrimitive.FeedbackPositive>
            <button type="button" className="fox-thread-action-button" aria-label="这条回答需要改进" onClick={() => setFeedbackOpen(true)}>
              <ThumbsDown size={14} />
            </button>
            <ActionBarPrimitive.Reload asChild>
              <button type="button" className="fox-thread-action-button" aria-label="重新生成回复"><RotateCcw size={14} /></button>
            </ActionBarPrimitive.Reload>
          </ActionBarPrimitive.Root>
          <div className="fox-message-meta">
            <span>{assistantName || 'Fox 默认助手'}</span>
            <span>{replyTime}</span>
            {cancelled && <span className="fox-message-cancelled-meta">运行已取消</span>}
            {failed && <span className="fox-message-failure-meta">运行失败</span>}
          </div>
        </div>
      </MessagePrimitive.Root>
      <FoxFeedbackDialog open={feedbackOpen} onOpenChange={setFeedbackOpen} messageId={messageId} />
    </div>
  )
}

