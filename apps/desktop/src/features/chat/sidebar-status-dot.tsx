import './sidebar-status-dot.css'

export function SidebarStatusDot({ state, label }: { state?: 'unread' | 'approval' | 'error'; label?: string }) {
  if (!state) return null
  const description = label ?? { unread: '未读消息', approval: '等待你确认', error: '运行异常' }[state]
  return <span className="fox-sidebar-status-dot" data-status={state} role="status" aria-label={description} title={description} />
}
