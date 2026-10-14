import type { ComponentProps } from 'react'
import { cva, type VariantProps } from 'class-variance-authority'
import { cn } from '@/lib/utils'

function Empty({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="empty" className={cn('flex min-w-0 flex-col items-center justify-center gap-4 p-6 text-center', className)} {...props} />
}
function EmptyHeader({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="empty-header" className={cn('flex max-w-sm flex-col items-center gap-2', className)} {...props} />
}
const emptyMediaVariants = cva('flex shrink-0 items-center justify-center [&_svg]:shrink-0', { variants: { variant: { default: '', icon: 'size-9 rounded-[var(--fox-radius-control)] bg-muted text-muted-foreground [&_svg]:size-5' } }, defaultVariants: { variant: 'default' } })
function EmptyMedia({ className, variant = 'default', ...props }: ComponentProps<'div'> & VariantProps<typeof emptyMediaVariants>) {
  return <div data-slot="empty-media" data-variant={variant} className={cn(emptyMediaVariants({ variant, className }))} {...props} />
}
function EmptyTitle({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="empty-title" className={cn('text-[length:var(--fox-type-title)] font-medium', className)} {...props} />
}
function EmptyDescription({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="empty-description" className={cn('text-[length:var(--fox-type-meta)] leading-relaxed text-muted-foreground [&_a]:underline', className)} {...props} />
}
function EmptyContent({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="empty-content" className={cn('flex w-full max-w-sm min-w-0 flex-col items-center gap-4', className)} {...props} />
}
export { Empty, EmptyHeader, EmptyMedia, EmptyTitle, EmptyDescription, EmptyContent }
