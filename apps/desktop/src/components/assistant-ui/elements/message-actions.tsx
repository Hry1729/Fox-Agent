import { Copy, GitFork, RotateCcw, ThumbsDown, ThumbsUp } from 'lucide-react'
import './fox-elements.css'
import { MessageActions as UpstreamActions } from '../upstream/elements/message-actions'

export function MessageActions({ reaction, busy, running, onCopy, onFork, onPositive, onNegative, onRegenerate }: { reaction?: 'positive' | 'negative' | null; busy?: boolean; running?: boolean; onCopy: () => void; onFork: () => void; onPositive: () => void; onNegative: () => void; onRegenerate?: () => void }) {
  return <UpstreamActions className="fox-message-actions aui-message-actions" copied={false} reaction={reaction === 'positive' ? 'up' : reaction === 'negative' ? 'down' : null} regenerating={Boolean(running)} busy={busy} running={running} allowClearReaction={false} onCopy={onCopy} onReactionChange={value => value === 'up' ? onPositive() : value === 'down' ? onNegative() : undefined} onRegenerate={onRegenerate} onMore={onFork} moreLabel="从此消息创建分支" moreIcon={<GitFork size={14} />} />
}
