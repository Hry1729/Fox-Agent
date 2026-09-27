"use client";

import { cjk } from "@streamdown/cjk";
import { math } from "@streamdown/math";
import { memo, useState, type ComponentProps } from "react";
import { Streamdown, type CustomRendererProps, type LinkSafetyModalProps } from "streamdown";
import { ExternalLink } from "lucide-react";
import { notify as toast } from "@/features/notifications";
import { beautifulMermaidPlugin } from "./beautiful-mermaid-diagram";

import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  desktopClient,
  desktopRuntimeAvailable,
} from "@/features/conversations/api/desktop-client";
import { CollapsibleCodeBlock } from "./code-block";

export type MessageResponseProps = ComponentProps<typeof Streamdown>;

const labelUnspecifiedCodeFences = (markdown: string) => {
  const lines = markdown.split("\n");
  let openFence: { marker: "`" | "~"; length: number } | null = null;

  return lines.map((line) => {
    const match = line.match(/^(\s*)(`{3,}|~{3,})(.*)$/);
    if (!match) return line;

    const marker = match[2][0] as "`" | "~";
    const length = match[2].length;
    if (openFence) {
      if (marker === openFence.marker && length >= openFence.length && match[3].trim() === "") {
        openFence = null;
      }
      return line;
    }

    openFence = { marker, length };
    return match[3].trim() === "" ? `${match[1]}${match[2]}output` : line;
  }).join("\n");
};

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
  "output",
  "stdout",
  "stderr",
  "console",
  "ansi",
  "log",
  "logs",
  "terminal",
  "plaintext",
  "cmd",
  "bat",
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
  // Same DiagramPlugin contract with the beautiful-mermaid engine; the stock renderer
  // stays the fallback for diagram families it does not implement.
  mermaid: beautifulMermaidPlugin,
  renderers: [{
    component: CollapsibleCodeRenderer,
    language: [...codeLanguages],
  }],
};

const ExternalLinkDialog = ({ isOpen, onClose, url }: LinkSafetyModalProps) => {
  const [opening, setOpening] = useState(false);

  const openLink = async () => {
    setOpening(true);
    try {
      if (desktopRuntimeAvailable) {
        await desktopClient.openExternalUrl(url);
      } else {
        window.open(url, "_blank", "noopener,noreferrer");
      }
      onClose();
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setOpening(false);
    }
  };

  return (
    <Dialog open={isOpen} onOpenChange={(open) => { if (!open) onClose(); }}>
      <DialogContent className="fox-external-link-dialog">
        <DialogHeader>
          <DialogTitle><ExternalLink size={17} />打开外部链接？</DialogTitle>
          <DialogDescription>将在系统默认浏览器中打开。</DialogDescription>
        </DialogHeader>
        <code title={url}>{url}</code>
        <DialogFooter>
          <Button type="button" variant="outline" size="sm" onClick={onClose}>取消</Button>
          <Button type="button" size="sm" disabled={opening} onClick={() => void openLink()}><ExternalLink size={14} />{opening ? "正在打开…" : "打开链接"}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

const linkSafety = {
  enabled: true,
  renderModal: (props: LinkSafetyModalProps) => <ExternalLinkDialog {...props} />,
};

export const MessageResponse = memo(
  ({ className, children, ...props }: MessageResponseProps) => {
    const normalizedChildren = typeof children === "string"
      ? labelUnspecifiedCodeFences(children)
      : children;

    return (
      <Streamdown
        className={cn(
          "size-full [&>*:first-child]:mt-0 [&>*:last-child]:mb-0",
          className
        )}
        linkSafety={linkSafety}
        plugins={streamdownPlugins}
        {...props}
      >
        {normalizedChildren}
      </Streamdown>
    );
  },
  (prevProps, nextProps) =>
    prevProps.children === nextProps.children &&
    nextProps.isAnimating === prevProps.isAnimating
);

MessageResponse.displayName = "MessageResponse";
export default MessageResponse;
