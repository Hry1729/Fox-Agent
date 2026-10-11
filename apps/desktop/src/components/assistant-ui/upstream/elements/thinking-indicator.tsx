"use client";

import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";
import { mono, ShimmerLabel } from "./surfaces";

export function ThinkingIndicator({
  label,
  active = true,
  showDot = active,
  elapsed,
  className,
  ...props
}: Omit<ComponentProps<"span">, "children" | "label" | "elapsed"> & {
  label: string;
  active?: boolean;
  showDot?: boolean;
  elapsed?: string;
}) {
  return (
    <span
      data-slot="thinking-indicator"
      className={cn(
        "text-muted-foreground flex items-center gap-2.5 text-sm",
        className,
      )}

      {...props}
    >
      {showDot && <span
        aria-hidden
        data-slot="thinking-indicator-dot"
        className={cn("size-1.5 shrink-0 rounded-full bg-[var(--fox-thinking-shimmer)] motion-reduce:animate-none", active && "animate-pulse")}
      />}
      <ShimmerLabel
        key={label}
        active={active}
        className="fade-in slide-in-from-bottom-1 animate-in relative inline-block min-w-0 truncate leading-tight duration-300 motion-reduce:animate-none"
      >
        {label}
      </ShimmerLabel>
      {elapsed !== undefined && (
        <span
          className={cn(mono, "text-muted-foreground shrink-0 tabular-nums")}
        >
          {elapsed}
        </span>
      )}
    </span>
  );
}

