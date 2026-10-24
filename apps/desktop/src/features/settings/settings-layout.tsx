import { useEffect, type ReactNode } from 'react'
import { cn } from '@/lib/utils'
import './settings-layout.css'

/**
 * Shared settings page frame: one page title with a single line of description,
 * then quiet groups (title + hairline) of hairline-divided rows. Rows put the
 * name and one sentence on the left and the control on the right; no cards.
 */
export function SettingsPageLayout({ title, description, actions, className, children }: { title: string; description: string; actions?: ReactNode; className?: string; children: ReactNode }) {
  return <div className={cn('fox-settings-page', className)}>
    <header className="fox-settings-page-head">
      <div><h1>{title}</h1><p>{description}</p></div>
      {actions && <div className="fox-settings-page-actions">{actions}</div>}
    </header>
    {children}
  </div>
}

export function SettingsGroup({ id, title, aside, className, children }: { id?: string; title: string; aside?: ReactNode; className?: string; children: ReactNode }) {
  return <section id={id} className={cn('fox-settings-group', className)} aria-label={title}>
    <div className="fox-settings-group-head"><h2>{title}</h2>{aside && <div className="fox-settings-group-aside">{aside}</div>}</div>
    {children}
  </section>
}

export function SettingsRow({ title, description, children, className, align = 'center' }: { title: ReactNode; description?: ReactNode; children?: ReactNode; className?: string; align?: 'center' | 'start' }) {
  return <div className={cn('fox-settings-row', align === 'start' && 'is-start', className)}>
    <div className="fox-settings-row-text"><b>{title}</b>{description && <small>{description}</small>}</div>
    {children !== undefined && <div className="fox-settings-row-control">{children}</div>}
  </div>
}

export function settingsSectionId(section: string) {
  return `fox-settings-section-${section}`
}

/** Scrolls a merged page to the section an old route (快捷键, 项目权限 …) pointed at. */
export function useScrollToSettingsSection(section: string | undefined) {
  useEffect(() => {
    if (!section) return
    let frame = 0
    let attempts = 0
    const reveal = () => {
      const target = document.getElementById(settingsSectionId(section))
      if (target) {
        target.scrollIntoView({ block: 'start' })
        return
      }
      if (attempts++ < 20) frame = window.requestAnimationFrame(reveal)
    }
    frame = window.requestAnimationFrame(reveal)
    return () => window.cancelAnimationFrame(frame)
  }, [section])
}
