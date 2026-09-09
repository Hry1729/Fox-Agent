"use client";

import { Badge } from "@/components/ui/badge";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import type { DynamicToolUIPart, ToolUIPart } from "ai";
import {
  CheckCircleIcon,
  ChevronDownIcon,
  CircleIcon,
  ClockIcon,
  WrenchIcon,
  XCircleIcon,
} from "lucide-react";
import type { ComponentProps, ReactNode } from "react";
import { isValidElement, memo, useMemo, useState } from "react";

import { CodeBlock } from "./code-block";

const TOOL_PAGE_SIZE = 6000;

// Large tool results must never enter the syntax highlighter: a spreadsheet
// can otherwise create hundreds of thousands of token spans in the chat.
const ToolValue = memo(function ToolValue({ value }: { value: unknown }) {
  const text = useMemo(() => typeof value === "string"
    ? value
    : JSON.stringify(value, null, 2) ?? String(value), [value]);
  const [page, setPage] = useState(0);
  const pages = Math.max(1, Math.ceil(text.length / TOOL_PAGE_SIZE));
  const currentPage = Math.min(page, pages - 1);
  if (text.length <= TOOL_PAGE_SIZE) return <CodeBlock code={text} language="json" />;
  return <div className="space-y-2 p-3">
    <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
      <span aria-live="polite">内容较长，分页查看 · {currentPage + 1} / {pages}</span>
      <div className="flex gap-3">
        <button type="button" className="disabled:opacity-40" disabled={currentPage === 0} onClick={() => setPage(currentPage - 1)}>上一页</button>
        <button type="button" className="disabled:opacity-40" disabled={currentPage === pages - 1} onClick={() => setPage(currentPage + 1)}>下一页</button>
      </div>
    </div>
    <pre key={currentPage} className="max-h-72 overflow-auto whitespace-pre-wrap break-all text-xs" tabIndex={0} aria-label="工具内容当前页">{text.slice(currentPage * TOOL_PAGE_SIZE, (currentPage + 1) * TOOL_PAGE_SIZE)}</pre>
  </div>;
});

export type ToolProps = ComponentProps<typeof Collapsible>;

export const Tool = ({ className, ...props }: ToolProps) => (
  <Collapsible
    className={cn("group not-prose mb-4 w-full rounded-md border", className)}
    {...props}
  />
);

export type ToolPart = ToolUIPart | DynamicToolUIPart;

export type ToolHeaderProps = {
  title?: string;
  className?: string;
} & (
  | { type: ToolUIPart["type"]; state: ToolUIPart["state"]; toolName?: never }
  | {
      type: DynamicToolUIPart["type"];
      state: DynamicToolUIPart["state"];
      toolName: string;
    }
);

const statusLabels: Record<ToolPart["state"], string> = {
  "approval-requested": "Awaiting Approval",
  "approval-responded": "Responded",
  "input-available": "Running",
  "input-streaming": "Pending",
  "output-available": "Completed",
  "output-denied": "Denied",
  "output-error": "Error",
};

const statusIcons: Record<ToolPart["state"], ReactNode> = {
  "approval-requested": <ClockIcon className="size-4 text-yellow-600" />,
  "approval-responded": <CheckCircleIcon className="size-4 text-blue-600" />,
  "input-available": <ClockIcon className="size-4 animate-pulse" />,
  "input-streaming": <CircleIcon className="size-4" />,
  "output-available": <CheckCircleIcon className="size-4 text-green-600" />,
  "output-denied": <XCircleIcon className="size-4 text-orange-600" />,
  "output-error": <XCircleIcon className="size-4 text-red-600" />,
};

export const getStatusBadge = (status: ToolPart["state"]) => (
  <Badge className="gap-1.5 rounded-full text-xs" variant="secondary">
    {statusIcons[status]}
    {statusLabels[status]}
  </Badge>
);

export const ToolHeader = ({
  className,
  title,
  type,
  state,
  toolName,
  ...props
}: ToolHeaderProps) => {
  const derivedName =
    type === "dynamic-tool" ? toolName : type.split("-").slice(1).join("-");

  return (
    <CollapsibleTrigger
      className={cn(
        "flex w-full items-center justify-between gap-4 p-3",
        className
      )}
      {...props}
    >
      <div className="flex items-center gap-2">
        <WrenchIcon className="size-4 text-muted-foreground" />
        <span className="font-medium text-sm">{title ?? derivedName}</span>
        {getStatusBadge(state)}
      </div>
      <ChevronDownIcon className="size-4 text-muted-foreground transition-transform group-data-[state=open]:rotate-180" />
    </CollapsibleTrigger>
  );
};

export type ToolContentProps = ComponentProps<typeof CollapsibleContent>;

export const ToolContent = ({ className, ...props }: ToolContentProps) => (
  <CollapsibleContent
    className={cn(
      "data-[state=closed]:fade-out-0 data-[state=closed]:slide-out-to-top-2 data-[state=open]:slide-in-from-top-2 space-y-4 p-4 text-popover-foreground outline-none data-[state=closed]:animate-out data-[state=open]:animate-in",
      className
    )}
    {...props}
  />
);

export type ToolInputProps = ComponentProps<"div"> & {
  input: ToolPart["input"];
};

export const ToolInput = ({ className, input, ...props }: ToolInputProps) => (
  <div className={cn("space-y-2 overflow-hidden", className)} {...props}>
    <h4 className="font-medium text-muted-foreground text-xs uppercase tracking-wide">
      参数
    </h4>
    <div className="rounded-md bg-muted/50">
      <ToolValue value={input} />
    </div>
  </div>
);

export type ToolOutputProps = ComponentProps<"div"> & {
  output: ToolPart["output"];
  errorText: ToolPart["errorText"];
};

export const ToolOutput = ({
  className,
  output,
  errorText,
  ...props
}: ToolOutputProps) => {
  if (!(output || errorText)) {
    return null;
  }

  let Output = <div>{output as ReactNode}</div>;

  if (typeof output === "object" && !isValidElement(output)) {
    Output = (
      <ToolValue value={output} />
    );
  } else if (typeof output === "string") {
    Output = <ToolValue value={output} />;
  }

  return (
    <div className={cn("space-y-2", className)} {...props}>
      <h4 className="font-medium text-muted-foreground text-xs uppercase tracking-wide">
        {errorText ? "错误" : "结果"}
      </h4>
      <div
        className={cn(
          "overflow-x-auto rounded-md text-xs [&_table]:w-full",
          errorText
            ? "bg-destructive/10 text-destructive"
            : "bg-muted/50 text-foreground"
        )}
      >
        {errorText && <div>{errorText}</div>}
        {Output}
      </div>
    </div>
  );
};
