import { useEffect, useRef, useState } from 'react'
import { CheckIcon, CopyIcon, GitFork, MailPlus } from 'lucide-react'
import { MessageActions } from '@/components/assistant-ui/upstream/elements/message-actions'
import { ghostButton, iconSwap, iconSwapIn, iconSwapOut } from '@/components/assistant-ui/upstream/elements/surfaces'
import { cn } from '@/lib/utils'
import './fox-message-actions.css'

type CopyAction = () => boolean | Promise<boolean>

function useCopyConfirmation(onCopy: CopyAction) {
  const [copied, setCopied] = useState(false)
  const [copying, setCopying] = useState(false)
  const pending = useRef(false)
  const alive = useRef(true)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  useEffect(() => {
    alive.current = true
    return () => { alive.current = false; clearTimeout(timer.current) }
  }, [])
  const copy = async () => {
    if (pending.current) return
    pending.current = true
    setCopying(true)
    try {
      if (!await onCopy() || !alive.current) return
      clearTimeout(timer.current)
      setCopied(true)
      timer.current = setTimeout(() => { if (alive.current) setCopied(false) }, 1800)
    } finally {
      pending.current = false
      if (alive.current) setCopying(false)
    }
  }
  return { copied, copying, copy }
}

// Official elements-message-actions / elements-surfaces provide the icons and
// copy/check swap. Fox supplies real callbacks and Chinese accessible labels.
export function FoxMessageActions({ reaction, busy, onCopy, onFork, onPositive, onNegative, onRegenerate }: {
  reaction: 'positive' | 'negative' | null
  busy: boolean
  onCopy: CopyAction
  onFork?: () => void
  onPositive: () => void
  onNegative: () => void
  onRegenerate?: () => void
}) {
  const { copied, copying, copy } = useCopyConfirmation(onCopy)
  return <MessageActions className="fox-thread-message-actions" copied={copied}
    reaction={reaction === 'positive' ? 'up' : reaction === 'negative' ? 'down' : null}
    regenerating={false} busy={busy} allowClearReaction={false} aria-busy={copying}
    onCopy={() => void copy()} onReactionChange={(value) => value === 'up' ? onPositive() : value === 'down' ? onNegative() : undefined}
    onRegenerate={onRegenerate} onMore={onFork} moreIcon={<GitFork aria-hidden="true" className="size-3.5" />} moreLabel="从此消息创建分支" />
}

export function FoxCopyMessageButton({ onCopy }: { onCopy: CopyAction }) {
  const { copied, copying, copy } = useCopyConfirmation(onCopy)
  return <button type="button" className={cn(ghostButton, 'fox-thread-action-button fox-thread-copy-button', copied && 'is-copied')}
    aria-label={copied ? '已复制消息' : '复制消息'} title={copied ? '已复制消息' : '复制消息'} aria-busy={copying} disabled={copying} onClick={() => void copy()}>
    <CopyIcon aria-hidden="true" className={cn(iconSwap, 'size-3.5', copied ? iconSwapOut : iconSwapIn)} />
    <CheckIcon aria-hidden="true" className={cn(iconSwap, 'size-3.5', copied ? iconSwapIn : iconSwapOut)} />
  </button>
}

/** C06/K6: mark a contact-sent visible bubble unread from its position onward. */
export function FoxMarkUnreadButton({ onMark }: { onMark: () => void | Promise<unknown> }) {
  const [marked, setMarked] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  useEffect(() => () => clearTimeout(timer.current), [])
  const mark = () => {
    void Promise.resolve(onMark()).then(() => {
      setMarked(true)
      clearTimeout(timer.current)
      timer.current = setTimeout(() => setMarked(false), 1800)
    }).catch(() => {})
  }
  return <button type="button" className={cn(ghostButton, 'fox-thread-action-button fox-thread-unread-button', marked && 'is-marked')}
    aria-label={marked ? '已标为未读' : '标为未读'} title={marked ? '已标为未读' : '标为未读'} onClick={mark}>
    <MailPlus aria-hidden="true" className={cn(iconSwap, 'size-3.5', marked ? iconSwapOut : iconSwapIn)} />
    <CheckIcon aria-hidden="true" className={cn(iconSwap, 'size-3.5', marked ? iconSwapIn : iconSwapOut)} />
  </button>
}
