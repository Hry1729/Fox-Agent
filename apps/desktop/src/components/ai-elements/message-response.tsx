"use client";

import { cjk } from "@streamdown/cjk";
import { math } from "@streamdown/math";
import { mermaid } from "@streamdown/mermaid";
import { memo, type ComponentProps } from "react";
import { Streamdown, type CustomRendererProps } from "streamdown";

import { cn } from "@/lib/utils";
import { CollapsibleCodeBlock } from "./code-block";

export type MessageResponseProps = ComponentProps<typeof Streamdown>;

const codeFilename = (meta?: string) =>
  meta?.match(/(?:title|filename)=["']([^"']+)["']/)?.[1];

const CollapsibleCodeRenderer = ({
  code,
  language,
  isIncomplete,
  meta,
}: CustomRendererProps) => (
  <CollapsibleCodeBlock
    className="fox-message-code-block"
    code={code}
    filename={codeFilename(meta)}
    isIncomplete={isIncomplete}
    language={language || "text"}
    showLineNumbers
  />
);

const codeLanguages = [
  "",
  "text",
  "typescript",
  "ts",
  "javascript",
  "js",
  "tsx",
  "jsx",
  "python",
  "py",
  "rust",
  "rs",
  "json",
  "bash",
  "sh",
  "shell",
  "shellscript",
  "powershell",
  "ps1",
  "markdown",
  "md",
  "html",
  "css",
  "sql",
  "yaml",
  "yml",
  "diff",
] as const;

const streamdownPlugins = {
  cjk,
  math,
  mermaid,
  renderers: [{
    component: CollapsibleCodeRenderer,
    language: [...codeLanguages],
  }],
};

export const MessageResponse = memo(
  ({ className, ...props }: MessageResponseProps) => (
    <Streamdown
      className={cn(
        "size-full [&>*:first-child]:mt-0 [&>*:last-child]:mb-0",
        className
      )}
      plugins={streamdownPlugins}
      {...props}
    />
  ),
  (prevProps, nextProps) =>
    prevProps.children === nextProps.children &&
    nextProps.isAnimating === prevProps.isAnimating
);

MessageResponse.displayName = "MessageResponse";
export default MessageResponse;
