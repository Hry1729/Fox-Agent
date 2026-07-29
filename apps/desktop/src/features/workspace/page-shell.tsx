import type { ReactNode } from 'react'
import { PanelLeftClose } from 'lucide-react'
import { Button } from '@/components/ui/button'

export function WorkspacePage({ title, subtitle, sidebarCollapsed, onSidebar, onBack, actions, className, children }: { title: string; subtitle?: string; sidebarCollapsed: boolean; onSidebar: () => void; onBack?: () => void; actions?: ReactNode; className?: string; children: ReactNode }) {
  void onBack
  return (
    <div className={`fox-page-shell ${className ?? ''}`}>
      <header className="fox-page-topbar">
        <div className="fox-page-topbar-main">
          {!sidebarCollapsed && <Button type="button" variant="ghost" size="icon" className="fox-icon-button" onClick={onSidebar} aria-label="收起侧边栏"><PanelLeftClose size={16} /></Button>}
          <div><strong title={title}>{title}</strong>{subtitle && <span>{subtitle}</span>}</div>
        </div>
        <div className="fox-page-topbar-actions">{actions}</div>
      </header>
      <div className="fox-page-scroll">{children}</div>
    </div>
  )
}
