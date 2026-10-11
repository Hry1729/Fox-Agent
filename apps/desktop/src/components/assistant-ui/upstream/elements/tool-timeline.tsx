"use client";

import { ChevronRightIcon, type LucideIcon } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import {
  CollapsibleRoot as Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./collapsible-root";
import { collapsePanel, ShimmerLabel, SwapLabel } from "./surfaces";
import { take } from "../utils/range";

export interface TimelineStep {
  id?: string;
  verb: string;
  chip: string;
  icon: LucideIcon;
  content?: ReactNode;
}

export interface TimelineStat {
  file: string;
  added?: number;
  removed?: number;
}

export interface ToolTimelineProps {
  steps: readonly TimelineStep[];
  visibleSteps: number;
  streaming: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  restingLabel: string;
  activeLabel: string;
  stats: TimelineStat[];
  className?: string;
  leading?: ReactNode;
  groupIcon?: ReactNode;
  renderContent?: (content: ReactNode) => ReactNode;
}

export function ToolTimeline({
  steps,
  visibleSteps,
  streaming,
  open,
  onOpenChange,
  restingLabel,
  activeLabel,
  stats,
  className,
  leading,
  groupIcon,
  renderContent = content => content,
}: ToolTimelineProps) {
  const [visited, setVisited] = useState(open);
  useEffect(() => { if (open) setVisited(true); }, [open]);
  return (
    <Collapsible
      data-slot="tool-timeline"
      open={open}
      onOpenChange={onOpenChange}
      className={cn("w-full max-w-sm", className)}
    >
      <div className="fox-runtime-process-head">{leading}<CollapsibleTrigger className="fox-element-timeline-trigger fox-chain-of-thought-header group/trigger text-muted-foreground hover:text-foreground/90 flex items-center gap-1.5 rounded-md py-1 text-[13.5px] transition-colors outline-none">
        <ChevronRightIcon className="size-3.5 shrink-0 opacity-60 transition-transform duration-200 ease-[cubic-bezier(0.32,0.72,0,1)] group-data-open/trigger:rotate-90 group-data-panel-open/trigger:rotate-90 motion-reduce:transition-none" />
        {groupIcon}
        <SwapLabel
          active={streaming ? 0 : 1}
          className={cn("fox-element-timeline-summary fox-runtime-process-summary text-start tabular-nums", streaming && "is-running")}
        >
          <ShimmerLabel
            active={streaming}
            className="relative inline-block leading-none"
          >
            {activeLabel}
          </ShimmerLabel>
          <>{restingLabel}</>
        </SwapLabel>
      </CollapsibleTrigger></div>
      {(visited || open) && <CollapsibleContent forceMount hidden={!open} aria-hidden={!open} inert={!open} className={cn(collapsePanel, "fox-chain-of-thought-content fox-runtime-process-disclosure outline-none")}>
        {renderContent(<div className="fox-element-timeline-steps flex min-w-0 flex-col gap-2.5 pt-2.5">
          {take(steps, visibleSteps).map((step, index, shown) => {
            const Icon = step.icon;
            const active = streaming && index === shown.length - 1;

            return step.content !== undefined ? <div key={step.id ? `id:${step.id}` : index} className="min-w-0">{step.content}</div> : (
              <div
                key={step.id ? `id:${step.id}` : index}
                className="fade-in slide-in-from-bottom-1 animate-in fill-mode-both text-muted-foreground flex items-center gap-2 text-[13.5px] duration-300"
              >
                <Icon className="text-muted-foreground size-3.5 shrink-0" />
                <ShimmerLabel
                  active={active}
                  className="relative inline-block leading-none"
                >
                  {step.verb}
                </ShimmerLabel>
                <span className="bg-foreground/[0.06] text-foreground/70 rounded-md px-1.5 py-0.5 font-mono text-[11px]">
                  {step.chip}
                </span>
              </div>
            );
          })}
          {stats.length > 0 && (
            <div className="flex flex-wrap gap-1.5 pt-1">
              {stats.map((stat) => (
                <span
                  key={stat.file}
                  className="bg-foreground/[0.06] text-foreground/70 inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 font-mono text-[11px]"
                >
                  <span>{stat.file}</span>
                  {stat.added !== undefined && (
                    <span className="text-emerald-600 dark:text-emerald-400">
                      +{stat.added}
                    </span>
                  )}
                  {stat.removed !== undefined && (
                    <span className="text-red-600 dark:text-red-400">
                      −{stat.removed}
                    </span>
                  )}
                </span>
              ))}
            </div>
          )}
        </div>)}
      </CollapsibleContent>}
    </Collapsible>
  );
}

