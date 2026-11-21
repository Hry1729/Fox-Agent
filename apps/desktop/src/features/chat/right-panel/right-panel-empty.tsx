import type { LucideIcon } from 'lucide-react'

export function RightPanelEmpty({ icon: Icon, children }: { icon: LucideIcon; children: string }) {
  return <div className="fox-panel-empty">
    <div className="fox-panel-empty-content">
      <Icon aria-hidden="true" />
      <p>{children}</p>
    </div>
  </div>
}
