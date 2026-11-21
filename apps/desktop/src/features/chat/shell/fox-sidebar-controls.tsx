import { createContext, useContext, useEffect, useId, useRef, useState, type ComponentProps } from 'react'
import { LayoutGroup, motion, useReducedMotion } from 'motion/react'
import { motionTokens } from '@/components/arc/lib/motion-tokens'
import * as RadixTabs from '@radix-ui/react-tabs'
import { ChevronDown } from 'lucide-react'
import type { UserMenuProps } from '@/components/arc/user-menu/user-menu'
import { deferredModule } from '@/components/fox-ui/use-deferred-module'
import userStyles from '@/components/arc/user-menu/user-menu.module.css'

const menu = deferredModule(() => import('@/components/arc/user-menu/user-menu'))
const ActiveModeContext = createContext<string | undefined>(undefined)
export function SidebarModeTabs({ value, onValueChange, children }: ComponentProps<typeof RadixTabs.Root>) {
  const id = useId()
  return <ActiveModeContext.Provider value={value}><LayoutGroup id={id}><RadixTabs.Root value={value} onValueChange={onValueChange} className="fox-mode-tabs fox-arc-mode-tabs"><RadixTabs.List data-slot="tabs-list" aria-label="工作区模式">{children}</RadixTabs.List></RadixTabs.Root></LayoutGroup></ActiveModeContext.Provider>
}
export function SidebarModeTab({ value, children, onPointerDown, onKeyDown, onBlur, ...props }: ComponentProps<typeof RadixTabs.Trigger>) {
  const active = useContext(ActiveModeContext)
  const reduced = useReducedMotion()
  return <RadixTabs.Trigger {...props} value={value} data-value={value} data-slot="tabs-trigger"
    onPointerDown={event => { event.currentTarget.dataset.pointerFocus = 'true'; onPointerDown?.(event) }}
    onKeyDown={event => { delete event.currentTarget.dataset.pointerFocus; onKeyDown?.(event) }}
    onBlur={event => { delete event.currentTarget.dataset.pointerFocus; onBlur?.(event) }}>
    {active === value && <motion.span className="fox-mode-selection" layoutId="mode-selection" transition={reduced ? { duration: 0 } : { duration: motionTokens.duration.standard, ease: motionTokens.ease.standard }} aria-hidden="true" />}<span className="fox-mode-content">{children}</span>
  </RadixTabs.Trigger>
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
