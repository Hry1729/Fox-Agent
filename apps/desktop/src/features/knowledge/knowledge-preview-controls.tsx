import type { ComponentProps, ReactNode } from 'react'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'

export function PreviewToolbar({ children }: { children: ReactNode }) {
  return <div className="fox-preview-toolbar">{children}</div>
}

export function PreviewIconButton({
  label,
  active = false,
  ...props
}: Omit<ComponentProps<'button'>, 'children'> & {
  label: string
  active?: boolean
  children: ReactNode
}) {
  const { children, ...buttonProps } = props
  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            className={`fox-preview-icon-button ${active ? 'is-active' : ''}`}
            aria-label={label}
            {...buttonProps}
          >
            {children}
          </button>
        </TooltipTrigger>
        <TooltipContent><p>{label}</p></TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}
