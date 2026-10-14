import { createContext, useContext, useEffect, useRef, useState, type ComponentProps } from 'react'
import * as RadixTabs from '@radix-ui/react-tabs'
import { ChevronDown } from 'lucide-react'
import type { UserMenuProps } from '@/components/arc/user-menu/user-menu'
import { deferredModule } from '@/components/fox-ui/use-deferred-module'
import tabStyles from '@/components/arc/tabs/tabs.module.css'
import userStyles from '@/components/arc/user-menu/user-menu.module.css'

const tabs = deferredModule(() => import('@/components/arc/tabs/tabs'))
const ActiveTabContext = createContext<string | undefined>(undefined)
const TabsContext = createContext<ReturnType<typeof tabs.useModule>>(undefined)
const menu = deferredModule(() => import('@/components/arc/user-menu/user-menu'))
export function SidebarModeTabs({ value, onValueChange, children }: ComponentProps<typeof RadixTabs.Root>) {
  const loaded = tabs.useModule()
  const content = <TabsContext.Provider value={loaded}><ActiveTabContext.Provider value={value}>{children}</ActiveTabContext.Provider></TabsContext.Provider>
  if (loaded) return <loaded.Tabs value={value} onValueChange={onValueChange} className="fox-mode-tabs fox-arc-mode-tabs"><loaded.TabsList aria-label="工作区模式">{content}</loaded.TabsList></loaded.Tabs>
  return <RadixTabs.Root value={value} onValueChange={onValueChange} className={`${tabStyles.root} fox-mode-tabs fox-arc-mode-tabs`} onPointerEnter={tabs.preload} onFocusCapture={tabs.preload}><div className={tabStyles.listShell}><div className={tabStyles.viewport}><RadixTabs.List className={tabStyles.list} aria-label="工作区模式">{content}</RadixTabs.List></div></div></RadixTabs.Root>
}
export function SidebarModeTab({ value, children, className, ...props }: ComponentProps<typeof RadixTabs.Trigger>) {
  const loaded = useContext(TabsContext)
  const active = useContext(ActiveTabContext)
  if (loaded) return <loaded.TabsTrigger value={value} className={className} {...props}>{children}</loaded.TabsTrigger>
  // Merge rather than replace: a bare `className={tabStyles.trigger}` drops any
  // caller class, which is how the fox-* sizing rules go missing on this path.
  return <RadixTabs.Trigger {...props} value={value} data-value={value} className={[tabStyles.trigger, className].filter(Boolean).join(' ')}>{active === value && <span className={tabStyles.selection} aria-hidden="true" />}<span className={tabStyles.triggerLabel}>{children}</span></RadixTabs.Trigger>
}
export function UserMenu(props: UserMenuProps) {
  const loaded = menu.useModule()
  const [innerOpen, setInnerOpen] = useState(props.defaultOpen ?? false)
  const [failedAvatar, setFailedAvatar] = useState<string>()
  const open = props.open ?? innerOpen
  const focusPending = useRef<'first' | 'last' | null>(null)
  useEffect(() => {
    if (!loaded || !open || !focusPending.current) return
    const pending = focusPending.current
    let frame: number
    let attempts = 0
    const started = performance.now()
    const focus = () => {
      const panel = document.querySelector<HTMLElement>(`[role="menu"].${props.panelClassName?.split(' ')[0] ?? userStyles.panel}`)
      const stops = panel?.querySelectorAll<HTMLElement>('[data-stop]')
      const stop = pending === 'last' ? stops?.[stops.length - 1] : stops?.[0]
      if (stop) stop.focus({ preventScroll: true })
      // Arc hydrates its portal one render later; other arriving leaves may still replace the focused fallback.
      if (++attempts < 60 && (!stop || performance.now() - started < 300)) frame = requestAnimationFrame(focus)
      else focusPending.current = null
    }
    frame = requestAnimationFrame(focus)
    return () => cancelAnimationFrame(frame)
  }, [loaded, open, props.panelClassName])
  const onOpenChange = (next: boolean) => { setInnerOpen(next); props.onOpenChange?.(next) }
  if (loaded) return <loaded.UserMenu {...props} open={open} onOpenChange={onOpenChange} />
  const { user, showName, className, triggerLabel } = props
  const initials = user.name.trim().split(/\s+/).slice(0, 2).map(part => part[0]?.toUpperCase()).join('')
  return <div className={userStyles.root}><button ref={props.ref} type="button" className={[userStyles.trigger, className].filter(Boolean).join(' ')} data-state={open ? 'open' : 'closed'} data-name={showName || undefined} aria-haspopup="menu" aria-expanded={open} aria-label={triggerLabel ?? `Account menu, ${user.name}`} onPointerEnter={menu.preload} onFocus={menu.preload} onClick={() => { menu.preload(); focusPending.current = 'first'; onOpenChange(!open) }} onKeyDown={event => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') { event.preventDefault(); menu.preload(); focusPending.current = event.key === 'ArrowUp' ? 'last' : 'first'; onOpenChange(true) }
    if (event.key === 'Escape') onOpenChange(false)
  }}><span className={userStyles.face} data-size="sm">{user.avatarSrc && failedAvatar !== user.avatarSrc ? <img className={userStyles.portrait} src={user.avatarSrc} srcSet={user.avatarSrcSet} alt="" decoding="async" draggable={false} onError={() => setFailedAvatar(user.avatarSrc)} /> : <span className={`${userStyles.portrait} ${userStyles.initials}`}>{initials}</span>}{(props.status || props.onStatusChange) && <span className={userStyles.dot} data-status={props.status ?? props.defaultStatus ?? 'available'} aria-hidden="true" />}</span>{showName && <><span className={userStyles.triggerName}>{user.name}</span><ChevronDown className={userStyles.chevron} size={16} strokeWidth={1.75} aria-hidden="true" /></>}</button></div>
}
