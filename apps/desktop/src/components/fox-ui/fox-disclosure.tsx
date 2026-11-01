// 带样式的折叠，用来替换原生 details。键盘和 aria-expanded 由 Radix 触发器负责。
// 收起时内容仍留在 DOM 里（hidden），和原来的 details 一样能被读到，只是不显示。
import { useState, type ReactNode } from 'react'
import { ChevronRight } from 'lucide-react'
import { Collapsible, CollapsibleTrigger } from '@/components/ui/collapsible'

export function FoxDisclosure({
  summary,
  children,
  className,
  defaultOpen = false,
  open,
  onOpenChange,
  id,
}: {
  summary: ReactNode
  children: ReactNode
  className?: string
  defaultOpen?: boolean
  open?: boolean
  onOpenChange?: (open: boolean) => void
  id?: string
}) {
  const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen)
  const isOpen = open ?? uncontrolledOpen
  const setOpen = (next: boolean) => {
    if (open === undefined) setUncontrolledOpen(next)
    onOpenChange?.(next)
  }
  return (
    <Collapsible
      id={id}
      className={className ? `fox-disclosure ${className}` : 'fox-disclosure'}
      open={isOpen}
      onOpenChange={setOpen}
    >
      <CollapsibleTrigger className="fox-disclosure-trigger">
        <ChevronRight className="fox-disclosure-chevron" aria-hidden="true" />
        <span className="fox-disclosure-summary">{summary}</span>
      </CollapsibleTrigger>
      <div className="fox-disclosure-panel" hidden={!isOpen}>{children}</div>
    </Collapsible>
  )
}
