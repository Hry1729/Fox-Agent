"use client";

import type { ComponentProps, ReactNode } from "react";
import { KeyRoundIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import { field, inkButton, mono, paper } from "./surfaces";
import { useReceiptFocus } from "./receipt-focus";

export type GrantScope = "once" | "conversation" | "session" | "always" | "denied";
const scopeLabels: Record<GrantScope, string> = { once: "只允许这一次", conversation: "本次对话内允许", session: "本次会话", always: "始终允许", denied: "拒绝" };

export function PermissionGrant({
  capability,
  requester,
  reach,
  scope,
  onGrant,
  className,
  reachClassName,
  allowedScopes = ["denied", "session", "always"],
  busy = false,
  detailsContent,
  description,
  ...props
}: Omit<
  ComponentProps<"div">,
  "children" | "capability" | "requester" | "reach" | "scope" | "onGrant"
> & {
  capability: string;
  requester: string;
  reach: readonly string[];
  scope: GrantScope | "pending";
  onGrant?: (scope: GrantScope) => void;
  reachClassName?: string;
  allowedScopes?: readonly GrantScope[];
  busy?: boolean;
  detailsContent?: ReactNode;
  description?: ReactNode;
}) {
  const { receiptRef, focusHandlers } = useReceiptFocus(props);
  return (
    <div
      data-slot="permission-grant"
      className={cn(
        paper,
        "flex w-full max-w-sm flex-col gap-3.5 rounded-[20px] p-4",
        className,
      )}

      {...props}
      {...focusHandlers}
    >
      <div className="flex items-center gap-2.5">
        <span className="bg-foreground/[0.05] text-muted-foreground flex size-7 shrink-0 items-center justify-center rounded-lg">
          <KeyRoundIcon className="size-3.5" />
        </span>
        <div className="flex min-w-0 flex-1 flex-col">
          <span className="truncate text-[13.5px] font-medium">
            {capability}
          </span>
          <span className="text-muted-foreground truncate text-xs">
            {requester} 请求授权
          </span>
        </div>
      </div>

      <div className="flex flex-col gap-1">
        <span className={cn(mono, "text-muted-foreground")}>授权范围</span>
        {reach.map((item) => (
          <span
            key={item}
            className={cn("text-foreground/60 flex items-baseline gap-2 text-xs", reachClassName)}
          >
            <span
              aria-hidden
              className="bg-foreground/20 size-1 rounded-full"
            />
            {item}
          </span>
        ))}
      </div>

      {description && <p className="aui-permission-description text-muted-foreground text-xs">{description}</p>}
      {detailsContent}
      <div className="fox-confirmation-actions flex min-h-8 items-center justify-end gap-2">
        {scope === "pending" ? (
          onGrant ? (
            allowedScopes.map((choice, index) => <button key={choice} type="button" disabled={busy} onClick={() => onGrant(choice)} className={cn("flex h-8 items-center rounded-full px-3 text-xs font-medium disabled:opacity-50", allowedScopes.includes("once") || allowedScopes.includes("conversation") ? choice === "once" && inkButton : choice !== "denied" && index === allowedScopes.length - 1 && inkButton, choice !== "once" && "text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground/90")}>
              {busy && choice === "once" ? "处理中…" : scopeLabels[choice]}
            </button>)
          ) : (
            <span
              key={scope}
              className={cn(
                field,
                mono,
                "fade-in animate-in text-muted-foreground rounded-full px-2.5 py-1.5 duration-300",
              )}
            >
              待确认
            </span>
          )
        ) : (
          <span
            key={scope}
            ref={receiptRef}
            tabIndex={-1}
            className={cn(
              field,
              mono,
              "fade-in animate-in text-muted-foreground rounded-full px-2.5 py-1.5 duration-300",
            )}
          >
            {scope === "denied" ? "已拒绝" : `已允许 · ${scopeLabels[scope]}`}
          </span>
        )}
      </div>
    </div>
  );
}

