"use client";

import { CheckIcon, ChevronRightIcon } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import {
  CollapsibleRoot as Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./collapsible-root";
import {
  collapsePanel,
  field,
  mono,
  ShimmerLabel,
  SwapLabel,
} from "./surfaces";

export interface ToolCallProps {
  label: ReactNode;
  activeLabel: ReactNode;
  query?: ReactNode;
  request?: ReactNode;
  result?: ReactNode;
  error?: string;
  icon?: ReactNode;
  ariaLabel?: string;
  running: boolean;
  completed?: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  className?: string;
}

export function ToolCall({
  label,
  activeLabel,
  query,
  request,
  result,
  running,
  completed = !running,
  open,
  onOpenChange,
  className,
  error,
  icon,
  ariaLabel,
}: ToolCallProps) {
  return (
    <Collapsible
      data-slot="tool-call"
      open={open}
      onOpenChange={onOpenChange}
      className={cn("w-full max-w-sm", className)}
    >
      <CollapsibleTrigger aria-label={ariaLabel} className="fox-element-tool-trigger group/trigger text-muted-foreground hover:text-foreground/90 flex w-full items-center gap-2 rounded-md py-1 text-[13.5px] transition-colors outline-none">
        <ChevronRightIcon className="size-3.5 shrink-0 opacity-60 transition-transform duration-200 ease-[cubic-bezier(0.32,0.72,0,1)] group-data-open/trigger:rotate-90 group-data-panel-open/trigger:rotate-90 motion-reduce:transition-none" />
        {icon}
        <SwapLabel
          active={running ? 0 : 1}
          className="shrink-0 text-start whitespace-nowrap"
        >
          <ShimmerLabel
            active={running}
            className="relative inline-block leading-none"
          >
            {activeLabel}
          </ShimmerLabel>
          <>{label}</>
        </SwapLabel>
        <span
          className={cn(
            mono,
            "bg-foreground/[0.06] text-foreground/70 min-w-0 truncate rounded-md px-1.5 py-0.5",
          )}
        >
          {query}
        </span>
        <span className="ms-auto flex w-4 shrink-0 items-center justify-end">
          {completed && !running && !error && (
            <CheckIcon className="fade-in zoom-in-90 animate-in size-3.5 text-emerald-500 duration-200" />
          )}
        </span>
      </CollapsibleTrigger>
      <CollapsibleContent className={cn(collapsePanel, "outline-none")}>
        <div className={cn(field, "mt-2 overflow-hidden rounded-2xl text-xs")}>
          {request !== undefined && <div className="px-3.5 pt-2.5 pb-2">
            <p className={cn(mono, "text-muted-foreground mb-1")}>请求</p>
            <div className="text-muted-foreground font-mono break-words">
              {request}
            </div>
          </div>}
          {result !== undefined && <><div className="bg-foreground/[0.06] mx-3.5 h-px" /><div className="px-3.5 pt-2 pb-2.5">
            <p className={cn(mono, "text-muted-foreground mb-1")}>结果</p>
            <div className="text-foreground/90 break-words">{result}</div>
          </div></>}
          {error && <p role="status" className="px-3.5 py-2 text-destructive">{error}</p>}
        </div>
      </CollapsibleContent>
    </Collapsible>
  );
}

