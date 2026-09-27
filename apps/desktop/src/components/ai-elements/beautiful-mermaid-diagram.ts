/**
 * beautiful-mermaid adapter for assistant answers.
 *
 * `beautiful-mermaid@1.1.3` (MIT, deps: elkjs + entities) renders six diagram families
 * synchronously. It is wired in as Streamdown's `mermaid` diagram plugin, so the whole
 * existing block pipeline — fence detection, streaming/incomplete handling, error UI,
 * copy and download controls — stays Streamdown's own; only the engine is swapped, and
 * anything beautiful-mermaid cannot handle falls through to the stock `@streamdown/mermaid`
 * renderer. One block is therefore never rendered by two engines.
 *
 * Capability set (each header rendered for real against the locked version, not assumed):
 *   flowchart / graph, stateDiagram(-v2), sequenceDiagram, classDiagram, erDiagram,
 *   xychart-beta  → beautiful-mermaid
 *   pie, gantt, journey, timeline, mindmap, gitGraph, quadrantChart … → stock renderer
 *   (those throw `Invalid mermaid header` upstream)
 *
 * Colours are CSS custom properties, which the library resolves in the SVG at paint time,
 * so a Fox theme switch re-paints without re-rendering or re-parsing the diagram.
 */
import { mermaid as stockMermaid, type MermaidInstance } from "@streamdown/mermaid";
import { renderMermaidSVG, type RenderOptions } from "beautiful-mermaid";
import type { DiagramPlugin } from "streamdown";

/** Headers beautiful-mermaid 1.1.3 actually accepts. */
const SUPPORTED_HEADER = /^\s*(?:flowchart|graph|stateDiagram(?:-v2)?|sequenceDiagram|classDiagram(?:-v2)?|erDiagram|xychart-beta)\b/i;

/**
 * The engine lays out synchronously (ELK, FakeWorker bypass), so a huge block would block
 * the main thread with no way to interrupt it. Bounding the input is the honest guard
 * available without moving layout into a Worker; blocks beyond it fall back to the stock
 * renderer instead of freezing the composer.
 */
export const DIAGRAM_MAX_CHARS = 12_000;
export const DIAGRAM_MAX_LINES = 300;

/** Patterns that must never reach the document from a generated diagram. */
const UNSAFE_PATTERNS: Array<[RegExp, string]> = [
  [/<script\b[^>]*>[\s\S]*?<\/script>/gi, "<script>"],
  [/<foreignObject\b[^>]*>[\s\S]*?<\/foreignObject>/gi, "<foreignObject>"],
  [/<iframe\b[^>]*>[\s\S]*?<\/iframe>/gi, "<iframe>"],
  [/\son[a-z]+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, "event handler attribute"],
  [/(?:javascript|vbscript)\s*:/gi, "script URL"],
  [/data:text\/html/gi, "data:text/html URL"],
];

/** True when the fenced source is a diagram family beautiful-mermaid implements. */
export function isBeautifulMermaidSource(source: string): boolean {
  return SUPPORTED_HEADER.test(source);
}

export function isWithinDiagramBudget(source: string): boolean {
  if (source.length > DIAGRAM_MAX_CHARS) return false;
  let lines = 1;
  for (let index = 0; index < source.length; index += 1) {
    if (source.charCodeAt(index) === 10 && (lines += 1) > DIAGRAM_MAX_LINES) return false;
  }
  return true;
}

/**
 * Strip anything executable from the generated markup. The SVG is produced by the library
 * from parsed text rather than taken from the model, so this is defence in depth: it must
 * leave text, theme and geometry untouched.
 */
export function sanitizeDiagramSvg(svg: string): { svg: string; removed: string[] } {
  let output = svg;
  const removed: string[] = [];
  for (const [pattern, label] of UNSAFE_PATTERNS) {
    if (!pattern.test(output)) continue;
    pattern.lastIndex = 0;
    output = output.replace(pattern, "");
    removed.push(label);
  }
  return { svg: output, removed };
}

const FONT_STACK = "HarmonyOS Sans SC, Segoe UI Variable, Segoe UI, system-ui, sans-serif";

/** Fox tokens, resolved by the browser at paint time so themes switch without a re-render. */
export function diagramRenderOptions(): RenderOptions {
  return {
    bg: "transparent",
    fg: "var(--fox-text)",
    line: "var(--fox-muted)",
    accent: "var(--fox-accent)",
    muted: "var(--fox-muted)",
    surface: "var(--fox-card)",
    border: "var(--fox-border)",
    font: FONT_STACK,
    padding: 24,
    transparent: true,
    interactive: true,
  };
}

export type DiagramRenderResult =
  | { ok: true; svg: string; removed: string[] }
  | { ok: false; reason: "unsupported" | "too-large" | "render-failed"; error?: string };

/** Route one fenced block: beautiful-mermaid where it applies, otherwise report why not. */
export function renderFoxDiagram(source: string): DiagramRenderResult {
  if (!isBeautifulMermaidSource(source)) return { ok: false, reason: "unsupported" };
  if (!isWithinDiagramBudget(source)) return { ok: false, reason: "too-large" };
  try {
    const { svg, removed } = sanitizeDiagramSvg(renderMermaidSVG(source, diagramRenderOptions()));
    if (!svg.trim()) return { ok: false, reason: "render-failed", error: "empty svg" };
    return { ok: true, svg, removed };
  } catch (error) {
    return { ok: false, reason: "render-failed", error: error instanceof Error ? error.message : String(error) };
  }
}

/**
 * Streamdown diagram plugin: same contract as `@streamdown/mermaid`, different engine.
 * Unsupported, oversized or failing blocks delegate to the stock renderer, which owns the
 * user-facing error surface.
 */
export const beautifulMermaidPlugin: DiagramPlugin = {
  name: "mermaid",
  type: "diagram",
  language: "mermaid",
  getMermaid(config): MermaidInstance {
    const fallback = stockMermaid.getMermaid(config);
    return {
      initialize: (next) => fallback.initialize(next),
      async render(id, source) {
        const result = renderFoxDiagram(source);
        if (result.ok) return { svg: result.svg };
        return fallback.render(id, source);
      },
    };
  },
};
