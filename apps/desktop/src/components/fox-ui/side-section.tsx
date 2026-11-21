import type { ComponentProps, ReactNode } from 'react'
import { cn } from '@/lib/utils'
import './side-section.css'

export function SideSection({ title, count, action, children, className }: { title: string; count?: number; action?: ReactNode; children: ReactNode; className?: string }) {
  return <section className={cn('fox-side-section', className)} aria-label={title}>
    <div className="fox-side-section-head"><h3>{title}</h3>{count !== undefined && <small>{count}</small>}<span className="fox-side-section-action">{action}</span></div>
    {children}
  </section>
}

export function SideCard({ className, ...props }: ComponentProps<'div'>) {
  return <div className={cn('fox-side-card', className)} {...props} />
}
