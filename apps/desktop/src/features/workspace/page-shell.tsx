import type { ReactNode } from 'react'

export function WorkspacePage({ title, subtitle, sidebarCollapsed, onSidebar, onBack, actions, className, children }: { title: string; subtitle?: string; sidebarCollapsed: boolean; onSidebar: () => void; onBack?: () => void; actions?: ReactNode; className?: string; children: ReactNode }) {
  void title
  void subtitle
  void sidebarCollapsed
  void onSidebar
  void onBack
  return (
    <div className={`fox-page-shell ${className ?? ''}`}>
      {actions && <div className="fox-page-floating-actions">{actions}</div>}
      <div className="fox-page-scroll">{children}</div>
    </div>
  )
}
