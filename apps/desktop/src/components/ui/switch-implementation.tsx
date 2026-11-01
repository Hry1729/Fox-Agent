import type * as React from 'react'
import { isValidElement } from 'react'
import type { Switch as RadixSwitch } from 'radix-ui'
import { Switch as AnimateSwitch, type SwitchProps as AnimateSwitchProps } from '@/components/animate-ui/components/base/switch'
import { cn } from '@/lib/utils'

type SwitchProps = React.ComponentProps<typeof RadixSwitch.Root> & { size?: 'sm' | 'default' }

/** Fox's existing checked/defaultChecked/onCheckedChange contract and compact dimensions. */
function Switch({ className, size = 'default', onCheckedChange, asChild, children, ...props }: SwitchProps) {
  return <AnimateSwitch
    {...props as Omit<AnimateSwitchProps, 'onCheckedChange'>}
    render={asChild && isValidElement<Record<string, unknown>>(children) ? children : undefined}
    data-slot="switch"
    data-size={size}
    onCheckedChange={(checked: boolean) => onCheckedChange?.(checked)}
    pressedWidth={size === 'sm' ? 14 : 16}
    thumbClassName="bg-[var(--fox-settings-switch-thumb)] shadow-none"
    className={cn('group/switch inline-flex border-transparent px-0 shadow-none motion-reduce:transition-none after:absolute after:-inset-x-3 after:-inset-y-2 focus-visible:ring-0 focus-visible:outline-none aria-invalid:border-destructive data-[unchecked]:bg-[var(--fox-settings-switch-off)] data-[unchecked]:border-[var(--fox-settings-switch-off)] dark:data-[unchecked]:bg-[var(--fox-settings-switch-off)] data-disabled:cursor-not-allowed data-disabled:opacity-100', className)}
  />
}

export { Switch, type SwitchProps }
