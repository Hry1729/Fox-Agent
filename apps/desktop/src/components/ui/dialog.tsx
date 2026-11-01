import * as React from "react"
import { Dialog as DialogPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import { XIcon } from "lucide-react"
import { keepOpenOnWindowChrome } from "@/components/ui/window-chrome"

function Dialog({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />
}

function DialogTrigger({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Portal>) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

function DialogClose({
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

function DialogOverlay({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Overlay>) {
  return (
    <DialogPrimitive.Overlay
      data-slot="dialog-overlay"
      className={cn(
        "fixed inset-x-0 bottom-0 top-[var(--fox-titlebar-height,0px)] isolate z-[100] duration-[var(--fox-duration-fast)] data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
        "bg-[var(--fox-scrim)] supports-backdrop-filter:backdrop-blur-xs",
        className
      )}
      {...props}
    />
  )
}

function DialogContent({
  className,
  children,
  showCloseButton = true,
  onPointerDownOutside,
  onInteractOutside,
  onFocusOutside,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & {
  showCloseButton?: boolean
}) {
  return (
    <DialogPortal>
      <DialogOverlay />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        className={cn(
          "fixed top-[calc(50%_+_var(--fox-titlebar-height,0px)_/_2)] left-1/2 z-[100] grid max-h-[calc(100vh_-_var(--fox-space-6xl)_-_var(--fox-titlebar-height,0px))] w-full max-w-[calc(100%_-_var(--fox-space-4xl))] -translate-x-1/2 -translate-y-1/2 gap-[var(--fox-dialog-gap)] overflow-hidden rounded-[var(--fox-radius-overlay)] border border-border bg-popover p-[var(--fox-dialog-padding)] text-[length:var(--fox-type-body)] leading-[1.7] text-popover-foreground shadow-[var(--fox-shadow)] duration-[var(--fox-duration-fast)] outline-none sm:max-w-sm data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
          className
        )}
        {...props}
        onPointerDownOutside={keepOpenOnWindowChrome(onPointerDownOutside)}
        onInteractOutside={keepOpenOnWindowChrome(onInteractOutside)}
        onFocusOutside={keepOpenOnWindowChrome(onFocusOutside)}
      >
        {children}
        {showCloseButton && (
          <DialogPrimitive.Close data-slot="dialog-close" asChild>
            <Button
              variant="ghost"
              className="absolute top-[var(--fox-space-m)] right-[var(--fox-space-m)]"
              size="icon"
              aria-label="关闭弹窗"
            >
              <XIcon
              />
              <span className="sr-only">关闭弹窗</span>
            </Button>
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Content>
    </DialogPortal>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex flex-col gap-[var(--fox-space-xs)]", className)}
      {...props}
    />
  )
}

function DialogFooter({
  className,
  showCloseButton = false,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  showCloseButton?: boolean
}) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn(
        "flex flex-col-reverse gap-[var(--fox-space-m)] sm:flex-row sm:items-center sm:justify-end",
        className
      )}
      {...props}
    >
      {children}
      {showCloseButton && (
        <DialogPrimitive.Close asChild>
          <Button variant="outline">Close</Button>
        </DialogPrimitive.Close>
      )}
    </div>
  )
}

function DialogTitle({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn("font-heading text-[length:var(--fox-type-title)] font-semibold", className)}
      {...props}
    />
  )
}

function DialogDescription({
  className,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn(
        "text-[length:var(--fox-type-body)] leading-[1.7] text-[var(--fox-muted)] *:[a]:underline *:[a]:underline-offset-3 *:[a]:hover:text-foreground",
        className
      )}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
}
