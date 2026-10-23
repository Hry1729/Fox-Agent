import { cloneElement, useRef, useState, type ComponentProps, type ReactElement, type ReactNode } from 'react'
import { AnimatedIconTrigger } from '@/components/fox-ui/animated-icons'
import { deferredModule } from '@/components/fox-ui/use-deferred-module'
const implementation = deferredModule(() => import('./fox-sidebar-motion-implementation'))

/** Content stays in the same aside while the official highlight layer arrives. */
export function SidebarMotion({ className, children }: { className: string; children: ReactNode }) {
  const loaded = implementation.useModule()
  const container = useRef<HTMLElement>(null)
  const [active, setActive] = useState<HTMLElement | null>(null)
  const updateActive = (target: EventTarget | null) => {
    const element = target instanceof Element ? target : null
    const next = element?.closest<HTMLElement>('[data-sidebar-motion-item]') ?? null
    // Keep the moving highlight across the gaps between settings groups.
    if (!next && element?.closest('[data-sidebar-motion-group]')) return
    setActive(next)
  }
  return <aside ref={container} className={className} data-slot="motion-highlight-container" style={{ position: 'relative', zIndex: 1 }} onPointerOver={event => { implementation.preload(); updateActive(event.target) }} onPointerOut={event => { updateActive(event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget) ? event.relatedTarget : null) }} onPointerLeave={() => setActive(null)} onFocusCapture={() => implementation.preload()}>
    {loaded && <loaded.SidebarHighlight container={container} active={active} />}
    {children}
  </aside>
}
export function SidebarMotionItem({ children, ...props }: Omit<ComponentProps<'div'>, 'children'> & { children: ReactElement }) {
  const child = children as ReactElement<ComponentProps<'div'> & { 'data-sidebar-motion-item'?: boolean; 'data-highlight'?: boolean }>
  return <span className="fox-sidebar-icon-trigger"><AnimatedIconTrigger>{cloneElement(child, { ...props, 'data-sidebar-motion-item': true, 'data-highlight': true })}</AnimatedIconTrigger></span>
}
