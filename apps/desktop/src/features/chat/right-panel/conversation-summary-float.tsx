import { X } from 'lucide-react'
import type { ConversationDetail } from '@/features/conversations/model/types'
import type { RightMode } from '../fox-shared'
import { ConversationSummary } from './conversation-summary'
import './conversation-summary-float.css'

export function ConversationSummaryFloat({ detail, modelName, onMode, onClose }: { detail: ConversationDetail | null; modelName: string; onMode: (mode: RightMode) => void; onClose: () => void }) {
  return <aside className="fox-summary-float" aria-label="对话摘要" onKeyDown={event => { if (event.key === 'Escape' && !event.defaultPrevented) { event.stopPropagation(); onClose() } }}>
    <header className="fox-summary-float-header"><span>对话摘要</span><button type="button" aria-label="关闭对话摘要" onClick={onClose}><X size={14} /></button></header>
    <ConversationSummary key={detail?.conversation.id || 'new'} detail={detail} modelName={modelName} onMode={mode => { if (mode === 'background') onClose(); onMode(mode) }} />
  </aside>
}
