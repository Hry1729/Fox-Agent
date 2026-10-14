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
    thumbClassName={size === 'sm' ? 'size-3' : 'size-3.5'}
    className={cn('group/switch inline-flex px-0 shadow-none motion-reduce:transition-none after:absolute after:-inset-x-3 after:-inset-y-2 focus-visible:ring-2 focus-visible:ring-ring/30 aria-invalid:border-destructive aria-invalid:ring-2 aria-invalid:ring-destructive/20 data-disabled:cursor-not-allowed data-disabled:opacity-50', size === 'sm' ? 'h-[14px] w-[24px]' : 'h-[16.6px] w-[28px]', className)}
  />
}

export { Switch, type SwitchProps }
