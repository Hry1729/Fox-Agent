"use client";

import type { ComponentProps } from "react";
import { CheckIcon, ThumbsDownIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import { field, inkButton, mono, paper } from "./surfaces";

export function FeedbackDialog({
  reasons,
  selected,
  note,
  sent,
  onToggleReason,
  onNoteChange,
  onSubmit,
  disabled = false,
  singleSelection = false,
  className,
  ...props
}: Omit<
  ComponentProps<"div">,
  | "children"
  | "reasons"
  | "selected"
  | "note"
  | "sent"
  | "onToggleReason"
  | "onNoteChange"
  | "onSubmit"
> & {
  reasons: readonly string[];
  selected: readonly string[];
  note: string;
  sent: boolean;
  onToggleReason?: (reason: string) => void;
  onNoteChange?: (note: string) => void;
  onSubmit?: () => void;
  disabled?: boolean;
  singleSelection?: boolean;
}) {
  return (
    <div
      data-slot="feedback-dialog"
      className={cn(
        paper,
        "flex w-full max-w-sm rounded-[20px] p-4",
        sent ? "items-center gap-2.5 text-[13.5px]" : "flex-col gap-3",
        className,
      )}

      {...props}
    >
      {/*
        Mounted whether or not the feedback has been sent, because a live region
        only announces a change that happens after it is already in the tree; a
        region created together with its text is the case AT is free to miss.
      */}
      <div
        role="status"
        className={
          sent
            ? "fade-in animate-in flex items-center gap-2.5 duration-300"
            : "sr-only"
        }
      >
        {sent && (
          <>
            <CheckIcon className="size-4 shrink-0 text-emerald-500" />
            反馈已保存，谢谢。
          </>
        )}
      </div>

      {sent ? null : (
        <>
          <div className="flex items-center gap-2.5">
            <span className="bg-foreground/[0.05] text-muted-foreground flex size-7 shrink-0 items-center justify-center rounded-lg">
              <ThumbsDownIcon className="size-3.5" />
            </span>
            <span className="text-[13.5px] font-medium">回答反馈</span>
            <span className={cn(mono, "text-muted-foreground ms-auto")}>
              可选
            </span>
          </div>

          <div className="flex flex-wrap gap-1.5" role={singleSelection ? "radiogroup" : undefined} aria-label="反馈分类">
            {reasons.map((reason, index) => {
              const active = selected.includes(reason);
              const className = cn(
                "rounded-full px-2.5 py-1 text-xs transition-[background-color,color,scale] duration-150",
                onToggleReason && "active:scale-[0.96]",
                active
                  ? "bg-foreground text-background"
                  : cn(
                      field,
                      "text-muted-foreground",
                      onToggleReason && "hover:text-foreground/90",
                    ),
              );
              return onToggleReason ? (
                <button
                  key={reason}
                  type="button"
                  disabled={disabled}
                  role={singleSelection ? "radio" : undefined}
                  aria-checked={singleSelection ? active : undefined}
                  aria-pressed={singleSelection ? undefined : active}
                  tabIndex={singleSelection ? active ? 0 : -1 : undefined}
                  onClick={() => onToggleReason(reason)}
                  onKeyDown={(event) => {
                    if (!singleSelection) return;
                    const direction = ["ArrowRight", "ArrowDown"].includes(event.key) ? 1 : ["ArrowLeft", "ArrowUp"].includes(event.key) ? -1 : 0;
                    if (!direction) return;
                    event.preventDefault();
                    const next = (index + direction + reasons.length) % reasons.length;
                    onToggleReason(reasons[next]);
                    (event.currentTarget.parentElement?.children[next] as HTMLButtonElement)?.focus();
                  }}
                  className={className}
                >
                  {reason}
                </button>
              ) : (
                <span
                  key={reason}
                  role="button"
                  aria-disabled="true"
                  aria-pressed={active}
                  className={className}
                >
                  {reason}
                </span>
              );
            })}
          </div>

          <textarea
            disabled={disabled}
            maxLength={1000}
            value={note}
            onChange={(event) => onNoteChange?.(event.target.value)}
            rows={2}
            placeholder="补充说明，最多 1000 字"
            aria-label="补充说明"
            className={cn(
              field,
              "text-foreground/80 placeholder:text-muted-foreground focus-visible:ring-foreground/20 resize-none rounded-xl px-3 py-2 text-xs outline-none focus-visible:ring-1",
            )}
          />

          {onSubmit && (
            <button
              type="button"
              disabled={disabled}
              onClick={onSubmit}
              className={cn(
                inkButton,
                "flex h-8 items-center justify-center self-end rounded-full px-3.5 text-xs font-medium",
              )}
            >
              {disabled ? '保存中…' : '提交反馈'}
            </button>
          )}
        </>
      )}
    </div>
  );
}

