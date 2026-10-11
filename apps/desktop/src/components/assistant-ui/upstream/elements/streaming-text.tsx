"use client";

import { createElement, type ComponentProps, useMemo } from "react";
import { cn } from "@/lib/utils";
import { take } from "../utils/range";

export interface Segment {
  text: string;
  mono?: boolean;
}

export function StreamingText({
  segments,
  count,
  streaming,
  inline = false,
  fresh = true,
  cursor = true,
  className,
  ...props
}: Omit<
  ComponentProps<"p">,
  "children" | "segments" | "count" | "streaming"
> & {
  segments: Segment[];
  count: number;
  streaming: boolean;
  inline?: boolean;
  fresh?: boolean;
  cursor?: boolean;
}) {
  const words = useMemo(
    () =>
      segments.flatMap((segment) =>
        (inline ? [segment.text] : segment.text.split(" "))
          .map((word) => ({ word, mono: segment.mono ?? false })),
      ),
    [segments, inline],
  );
  const shown = take(words, count);

  return createElement(inline ? "span" : "p", {
      "data-slot": "streaming-text",
      className: cn(!inline && "min-h-[8.5rem] max-w-sm text-sm leading-relaxed text-pretty", className),
      ...props,
    }, <>
      {shown.map(({ word, mono: isMono }, i) => {
        const isFresh = fresh && streaming && shown.length - 1 - i < 2;
        return (
          <span
            key={i}
            className={cn(isFresh && "fade-in animate-in fill-mode-both duration-500 motion-reduce:animate-none")}
          >
            <span
              className={cn(
                "transition-colors duration-700 motion-reduce:transition-none",
                isFresh && "text-brand aui-streaming-fresh",
                isMono &&
                  "bg-foreground/[0.06] rounded-md px-1.5 py-0.5 font-mono text-[0.85em]",
              )}
            >
              {word}
            </span>{inline ? "" : " "}
          </span>
        );
      })}
      {cursor && streaming && shown.length > 0 && (
        <StreamingCursor />
      )}
    </>);
}

export function StreamingCursor() {
  return <span aria-hidden className="-mb-0.5 ml-0.5 inline-block h-4 w-0.5 animate-pulse rounded-full bg-brand motion-reduce:animate-none" />;
}

