"use client";

import type { ComponentProps, ReactNode } from "react";
import {
  CheckIcon,
  CopyIcon,
  EllipsisIcon,
  RefreshCwIcon,
  ThumbsDownIcon,
  ThumbsUpIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { ghostButton, iconSwap, iconSwapIn, iconSwapOut } from "./surfaces";

export type Reaction = "up" | "down" | null;

export interface MessageActionsProps extends Omit<
  ComponentProps<"div">,
  "children"
> {
  copied: boolean;
  reaction: Reaction;
  regenerating: boolean;
  onCopy: () => void;
  onReactionChange: (reaction: Reaction) => void;
  onRegenerate?: () => void;
  onMore: () => void;
  moreLabel?: string;
  moreIcon?: ReactNode;
  busy?: boolean;
  running?: boolean;
  allowClearReaction?: boolean;
}

export function MessageActions({
  copied,
  reaction,
  regenerating,
  onCopy,
  onReactionChange,
  onRegenerate,
  onMore,
  moreLabel = '更多回复操作', moreIcon, busy = false, running = false, allowClearReaction = true,
  className,
  ...props
}: MessageActionsProps) {
  const buttonClassName = cn(ghostButton, "size-7");

  return (
    <div
      data-slot="message-actions"
      className={cn("flex items-center gap-1", className)}

      {...props}
    >
      <button
        type="button"
        aria-label={copied ? "已复制回复" : "复制回复"}
        onClick={onCopy}
        className={cn(
          buttonClassName,
          "grid place-items-center",
          copied && "text-emerald-500",
        )}
      >
        <CopyIcon
          className={cn(
            iconSwap,
            "size-3.5",
            copied ? iconSwapOut : iconSwapIn,
          )}
        />
        <CheckIcon
          className={cn(
            iconSwap,
            "size-3.5",
            copied ? iconSwapIn : iconSwapOut,
          )}
        />
      </button>
      <button
        type="button"
        aria-label="这条回答有帮助"
        disabled={running || busy}
        aria-pressed={reaction === "up"}
        onClick={() => onReactionChange(allowClearReaction && reaction === "up" ? null : "up")}
        className={cn(
          buttonClassName,
          reaction === "up" &&
            "bg-foreground/[0.06] text-foreground/90 dark:bg-foreground/[0.09]",
        )}
      >
        <ThumbsUpIcon className="size-3.5" />
      </button>
      <button
        type="button"
        aria-label="这条回答需要改进"
        disabled={running || busy}
        aria-pressed={reaction === "down"}
        onClick={() => onReactionChange(allowClearReaction && reaction === "down" ? null : "down")}
        className={cn(
          buttonClassName,
          reaction === "down" &&
            "bg-foreground/[0.06] text-foreground/90 dark:bg-foreground/[0.09]",
        )}
      >
        <ThumbsDownIcon className="size-3.5" />
      </button>
      {onRegenerate && <button
        type="button"
        aria-label="重新生成回复"
        disabled={running || busy}
        onClick={onRegenerate}
        className={buttonClassName}
      >
        <RefreshCwIcon
          className={cn(
            "size-3.5",
            regenerating && "animate-spin motion-reduce:animate-none",
          )}
        />
      </button>}
      <button
        type="button"
        aria-label={moreLabel}
        disabled={running}
        onClick={onMore}
        className={buttonClassName}
      >
        {moreIcon ?? <EllipsisIcon className="size-3.5" />}
      </button>
    </div>
  );
}

