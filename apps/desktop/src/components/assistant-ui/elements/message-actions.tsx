import { GitFork } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import './fox-elements.css'
import './message-actions.css'
import { MessageActions as UpstreamActions } from '../upstream/elements/message-actions'
import { notify as toast } from '@/features/notifications'

export async function copyMessageResponse(text: string): Promise<boolean> {
  try {
    if (!text.trim() || !navigator.clipboard?.writeText) throw new Error('Clipboard unavailable')
    await navigator.clipboard.writeText(text)
    toast.success('已复制回复')
    return true
  } catch {
    toast.error('复制失败，请重试')
    return false
  }
}

export function MessageActions({ reaction, busy, running, onCopy, onFork, onPositive, onNegative, onRegenerate }: { reaction?: 'positive' | 'negative' | null; busy?: boolean; running?: boolean; onCopy: () => void | boolean | Promise<void | boolean>; onFork: () => void; onPositive: () => void; onNegative: () => void; onRegenerate?: () => void }) {
  const [copied, setCopied] = useState(false)
  const [copying, setCopying] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const alive = useRef(true)
  useEffect(() => { alive.current = true; return () => { alive.current = false; clearTimeout(timer.current) } }, [])
  const copy = async () => {
    if (copying) return
    setCopying(true)
    try {
      const result = await onCopy()
      if (!alive.current || result === false) return
      clearTimeout(timer.current)
      setCopied(true)
      timer.current = setTimeout(() => { if (alive.current) setCopied(false) }, 1800)
    } finally { if (alive.current) setCopying(false) }
  }
  return <UpstreamActions className="fox-message-actions aui-message-actions" copied={copied} reaction={reaction === 'positive' ? 'up' : reaction === 'negative' ? 'down' : null} regenerating={Boolean(running)} busy={busy} running={running} allowClearReaction={false} onCopy={() => void copy()} onReactionChange={value => value === 'up' ? onPositive() : value === 'down' ? onNegative() : undefined} onRegenerate={onRegenerate} onMore={onFork} moreLabel="从此消息创建分支" moreIcon={<GitFork className="size-3.5" />} />
}
