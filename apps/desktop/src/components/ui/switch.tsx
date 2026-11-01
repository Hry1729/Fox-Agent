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
  return <BaseSwitch.Root {...props as Omit<ComponentProps<typeof BaseSwitch.Root>, 'onCheckedChange'>} render={asChild && isValidElement<Record<string, unknown>>(children) ? children : <button />} checked={current} onCheckedChange={change} data-slot="switch" data-size={size} className={cn('relative peer inline-flex shrink-0 items-center rounded-full border border-transparent bg-[var(--fox-settings-switch-off)] data-[checked]:bg-primary outline-none focus-visible:outline-none disabled:cursor-not-allowed disabled:opacity-100', className)}>
    <BaseSwitch.Thumb data-slot="switch-thumb" className="pointer-events-none block rounded-full bg-[var(--fox-settings-switch-thumb)]" style={{ transform: current ? 'translateX(var(--fox-settings-switch-travel))' : undefined }} />
  </BaseSwitch.Root>
}
export { Switch, type SwitchProps }
