import { cloneElement, useRef, useState, type ComponentProps, type ReactElement, type ReactNode } from 'react'
import { AnimatedIconTrigger } from '@/components/fox-ui/animated-icons'
import { deferredModule } from '@/components/fox-ui/use-deferred-module'
const implementation = deferredModule(() => import('./fox-sidebar-motion-implementation'))

/** Content stays in the same aside while the official highlight layer arrives. */
export function SidebarMotion({ className, children }: { className: string; children: ReactNode }) {
  const loaded = implementation.useModule()
  const container = useRef<HTMLElement>(null)
  const [active, setActive] = useState<HTMLElement | null>(null)
  const row = (target: EventTarget) => target instanceof Element ? target.closest<HTMLElement>('[data-sidebar-motion-item]') : null
  return <aside ref={container} className={className} data-slot="motion-highlight-container" style={{ position: 'relative', zIndex: 1 }} onPointerOver={event => { implementation.preload(); setActive(row(event.target)) }} onPointerOut={event => { setActive(event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget) ? row(event.relatedTarget) : null) }} onPointerLeave={() => setActive(null)} onFocusCapture={() => implementation.preload()}>
    {loaded && <loaded.SidebarHighlight container={container} active={active} />}
    {children}
  </aside>
}
export function SidebarMotionItem({ children, ...props }: Omit<ComponentProps<'div'>, 'children'> & { children: ReactElement }) {
  const child = children as ReactElement<ComponentProps<'div'> & { 'data-sidebar-motion-item'?: boolean; 'data-highlight'?: boolean }>
  return <span className="fox-sidebar-icon-trigger"><AnimatedIconTrigger>{cloneElement(child, { ...props, 'data-sidebar-motion-item': true, 'data-highlight': true })}</AnimatedIconTrigger></span>
}
