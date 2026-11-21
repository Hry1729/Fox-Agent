import { BookOpen, FileEdit, Folders, Globe2, MessagesSquare, Terminal } from 'lucide-react'
import { FloatingButtonGroup } from '@/components/arc/floating-button-group/floating-button-group'
import type { RightMode } from '../fox-shared'

export function RightQuickActions({ onMode }: { onMode: (mode: RightMode) => void }) {
  return <FloatingButtonGroup className="fox-chat-quick-actions" label="右侧快捷页面" orientation="vertical" variant="floating" size="sm" iconOnly tooltipSide="left" items={[
    { id: 'files', label: '文件', icon: <Folders />, onSelect: () => onMode('files') },
    { id: 'browser', label: '网页与网络', icon: <Globe2 />, onSelect: () => onMode('browser') },
    { id: 'agents', label: '协作助手', icon: <MessagesSquare />, onSelect: () => onMode('agents') },
    { type: 'separator', id: 'details' },
    { id: 'knowledge', label: '资料库', icon: <BookOpen />, onSelect: () => onMode('knowledge') },
    { id: 'changes', label: '更改', icon: <FileEdit />, onSelect: () => onMode('changes') },
    { id: 'terminal', label: '终端', icon: <Terminal />, onSelect: () => onMode('terminal') },
  ]} />
}
