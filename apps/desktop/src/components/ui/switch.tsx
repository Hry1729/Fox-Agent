import { isValidElement, useState, type ComponentProps } from 'react'
import { Switch as BaseSwitch } from '@base-ui-components/react/switch'
import type { SwitchProps } from './switch-implementation'
import { cn } from '@/lib/utils'
import { deferredModule } from '@/components/fox-ui/use-deferred-module'
const implementation = deferredModule(() => import('./switch-implementation'))
function Switch({ checked, defaultChecked = false, onCheckedChange, className, size = 'default', asChild, children, ...props }: SwitchProps) {
  const loaded = implementation.useModule()
  const [internal, setInternal] = useState(defaultChecked)
  const current = checked ?? internal
  const change = (next: boolean) => { setInternal(next); onCheckedChange?.(next) }
  if (loaded) return <loaded.Switch {...props} {...{ checked: current, onCheckedChange: change, className, size, asChild, children }} />
  return <BaseSwitch.Root {...props as Omit<ComponentProps<typeof BaseSwitch.Root>, 'onCheckedChange'>} render={asChild && isValidElement<Record<string, unknown>>(children) ? children : <button />} checked={current} onCheckedChange={change} data-slot="switch" data-size={size} className={cn('relative peer inline-flex shrink-0 items-center rounded-full border border-transparent bg-input data-[checked]:bg-primary outline-none focus-visible:ring-2 focus-visible:ring-ring/30 disabled:cursor-not-allowed disabled:opacity-50', size === 'sm' ? 'h-[14px] w-[24px]' : 'h-[16.6px] w-[28px]', className)}>
    <BaseSwitch.Thumb className={cn('block rounded-full bg-background pointer-events-none', size === 'sm' ? 'size-3' : 'size-3.5')} style={{ transform: current ? `translateX(${size === 'sm' ? 10 : 12}px)` : undefined }} />
  </BaseSwitch.Root>
}
export { Switch, type SwitchProps }
