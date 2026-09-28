/**
 * beautiful-mermaid adapter for assistant answers.
 *
 * `beautiful-mermaid@1.1.3` (MIT, deps: elkjs + entities) renders six diagram families
 * synchronously. It is wired in as Streamdown's `mermaid` diagram plugin, so the whole
 * existing block pipeline — fence detection, streaming/incomplete handling, error UI,
 * copy and download controls — stays Streamdown's own; only the engine is swapped.
 *
 * Capability set (each header rendered for real against the locked version):
 *   flowchart / graph, stateDiagram(-v2), sequenceDiagram, classDiagram, erDiagram,
 *   xychart-beta → beautiful-mermaid
 *   pie, gantt, journey, timeline, mindmap, gitGraph, quadrantChart … → stock renderer
 *   (beautiful-mermaid rejects those with `Invalid mermaid header`)
 *
 * Four invariants this module owns:
 *
 * 1. **Per-diagram id namespace.** The library emits document-global ids (`arrowhead`,
 *    `arrowhead-start`) plus one id per node, so two diagrams inlined in one document
 *    collide. Every `id` and every fragment reference (`url(#…)`, `href="#…"`) is rewritten
 *    with the block id Streamdown passes to `render(id, source)`, which keeps each diagram
 *    independent of another's mount, collapse or removal.
 * 2. **A budget checked before any layout engine runs.** Layout is synchronous (ELK over a
 *    fake worker), so an oversized source cannot be interrupted — and handing it to the
 *    *stock* engine instead would only move the stall. Over budget therefore renders
 *    nothing and reports why, so the caller can show the source.
 * 3. **A parse-based allowlist, not a list of scary strings.** Generated SVG is parsed and
 *    rebuilt: unknown elements/attributes are dropped, URL-valued attributes must be local
 *    fragments, and `style`/`<style>` URL targets are filtered. Parsing decodes entities
 *    first, so `java&#x73;cript:` is caught instead of passing through.
 * 4. **Fallback only where it is meant.** Unsupported families and in-budget render failures
 *    go to `@streamdown/mermaid`; one block is never laid out by two engines.
 */
import { mermaid as stockMermaid, type MermaidInstance } from "@streamdown/mermaid";
import { parseMermaid, renderMermaidSVG, type RenderOptions } from "beautiful-mermaid";
import type { DiagramPlugin } from "streamdown";

/** Headers beautiful-mermaid 1.1.3 actually accepts. */
const SUPPORTED_HEADER = /^\s*(?:flowchart|graph|stateDiagram(?:-v2)?|sequenceDiagram|classDiagram(?:-v2)?|erDiagram|xychart-beta)\b/i;

/** Families whose node/edge counts `parseMermaid` reports exactly. */
const PARSED_HEADER = /^\s*(?:flowchart|graph|stateDiagram(?:-v2)?)\b/i;

/**
 * Budget. Characters and lines bound the input; nodes and edges bound the work a layout
 * engine can be asked to do, because a short source can still describe a large graph. Edge
 * counting for families `parseMermaid` does not read is a documented, deliberately generous
 * heuristic over relationship/arrow operators.
 */
export const DIAGRAM_MAX_CHARS = 12_000;
export const DIAGRAM_MAX_LINES = 300;
export const DIAGRAM_MAX_NODES = 200;
export const DIAGRAM_MAX_EDGES = 300;

const EDGE_OPERATOR = /(?:<-->|<==>|-->|->>|-->>|--x|--o|-\.->|==>|---|\|\|--|\|\|\.\.|}\|\.\.|}\|--|}\.\.|\}o--|\|\{--)/g;

export function isBeautifulMermaidSource(source: string): boolean {
  return SUPPORTED_HEADER.test(source);
}

export type DiagramBudget =
  | { ok: true; nodes?: number; edges: number }
  | { ok: false; reason: "too-large"; limit: string };

export type DiagramParser = (source: string) => { nodes: Map<string, unknown>; edges: unknown[] };

/** Decide, cheaply and before any layout, whether a source may be rendered at all. */
export function checkDiagramBudget(source: string, options: { parse?: DiagramParser } = {}): DiagramBudget {
  if (source.length > DIAGRAM_MAX_CHARS) return { ok: false, reason: "too-large", limit: `源码超过 ${DIAGRAM_MAX_CHARS} 字符` };
  let lines = 1;
  for (let index = 0; index < source.length; index += 1) {
    if (source.charCodeAt(index) === 10 && (lines += 1) > DIAGRAM_MAX_LINES) {
      return { ok: false, reason: "too-large", limit: `源码超过 ${DIAGRAM_MAX_LINES} 行` };
    }
  }
  if (PARSED_HEADER.test(source)) {
    try {
      const graph = (options.parse ?? parseMermaid)(source);
      if (graph.nodes.size > DIAGRAM_MAX_NODES) return { ok: false, reason: "too-large", limit: `节点超过 ${DIAGRAM_MAX_NODES} 个` };
      if (graph.edges.length > DIAGRAM_MAX_EDGES) return { ok: false, reason: "too-large", limit: `连线超过 ${DIAGRAM_MAX_EDGES} 条` };
      return { ok: true, nodes: graph.nodes.size, edges: graph.edges.length };
    } catch {
      // A parse failure is a syntax problem, not a size problem: the renderer reports it.
      return { ok: true, edges: 0 };
    }
  }
  const edges = (source.match(EDGE_OPERATOR) ?? []).length;
  if (edges > DIAGRAM_MAX_EDGES) return { ok: false, reason: "too-large", limit: `连线超过 ${DIAGRAM_MAX_EDGES} 条` };
  return { ok: true, edges };
}

// ---------------------------------------------------------------------------------------
// SVG allowlist sanitizer + id namespacing
// ---------------------------------------------------------------------------------------

/** Element names the renderer emits, plus the standard shapes other families may use. */
const ALLOWED_TAGS = new Set([
  "svg", "g", "defs", "marker", "rect", "circle", "ellipse", "line", "polyline", "polygon",
  "path", "text", "tspan", "title", "desc", "style", "lineargradient", "radialgradient",
  "stop", "clippath", "mask", "pattern", "use", "symbol",
]);

/** Presentation/geometry attributes. Anything unlisted is dropped; `on*` always is. */
const ALLOWED_ATTRIBUTES = new Set([
  "id", "class", "style", "transform", "xmlns", "viewbox", "width", "height",
  "x", "y", "x1", "y1", "x2", "y2", "cx", "cy", "r", "rx", "ry", "d", "points", "offset",
  "dx", "dy", "fill", "fill-opacity", "fill-rule", "stroke", "stroke-width", "stroke-linecap",
  "stroke-linejoin", "stroke-dasharray", "stroke-dashoffset", "stroke-opacity", "opacity",
  "font-family", "font-size", "font-weight", "font-style", "letter-spacing", "text-anchor",
  "dominant-baseline", "alignment-baseline", "paint-order", "shape-rendering", "marker-start",
  "marker-mid", "marker-end", "markerwidth", "markerheight", "orient", "refx", "refy",
  "markerunits", "gradientunits", "gradienttransform", "spreadmethod", "patternunits",
  "patterncontentunits", "patterntransform", "clippathunits", "maskunits", "maskcontentunits",
  "stop-color", "stop-opacity", "preserveaspectratio", "href", "xlink:href", "clip-path",
  "clip-rule", "mask", "filter", "role", "aria-hidden", "aria-label", "focusable",
]);

/** Attributes that are always a reference and must stay a local fragment. */
const FRAGMENT_ATTRIBUTES = new Set(["href", "xlink:href"]);

/**
 * Attributes that *may* carry `url(…)` (and otherwise hold a plain colour, `none` or a
 * `var(…)` token). Only a real `url(…)` target is validated.
 */
const URL_FUNCTION_ATTRIBUTES = new Set(["fill", "stroke", "marker-start", "marker-mid", "marker-end", "clip-path", "mask", "filter"]);

const isLocalReference = (value: string) => /^#[\w:.-]+$/.test(value.trim());

/** `url(#id)` is fine; `url(https://…)`, `url(javascript:…)` and `url("data:…")` are not. */
const isLocalUrlFunction = (value: string) => {
  const trimmed = value.trim();
  if (!/^url\(/i.test(trimmed)) return false;
  const target = trimmed.slice(4, -1).trim().replace(/^["']|["']$/g, "");
  return isLocalReference(target);
};

/** CSS inside `style` / `<style>` may only reach local fragments; `@import` and external
 *  `url(…)` targets are removed. Fragment *renaming* is a separate step below, because a
 *  bare `#abc` in CSS may be a colour and must not be touched. */
export function sanitizeCssText(css: string): string {
  return css
    // `@import url(…)` must be removed whole: a naive `@import[^;]*` stops at the first `;`,
    // and a font URL query (`…wght@400;500;600;700&display=swap`) contains semicolons, which
    // used to leave a stray fragment behind in the stylesheet.
    .replace(/@import\s*(?:url\(\s*(?:"[^"]*"|'[^']*'|[^)]*)\s*\)|"[^"]*"|'[^']*')[^;]*;?/gi, "")
    .replace(/@import[^;]*;?/gi, "")
    .replace(/url\(\s*(?!#)[^)]*\)/gi, "none")
    .replace(/(?:javascript|vbscript)\s*:/gi, "");
}

/** Rewrite `url(#id)` inside CSS text to the diagram's namespace; leaves colours alone. */
export function rewriteCssFragmentIds(css: string, rename: Map<string, string>): string {
  if (rename.size === 0) return css;
  return css.replace(/url\(\s*#([\w:.-]+)\s*\)/gi, (whole, id: string) => (rename.has(id) ? `url(#${rename.get(id)})` : whole));
}

/** `url(#id)` inside an attribute value; a bare `#abc` is a colour and stays untouched. */
export function rewriteAttributeFragmentIds(value: string, rename: Map<string, string>): string {
  if (rename.size === 0) return value;
  return value.replace(/url\(\s*#([\w:.-]+)\s*\)/gi, (whole, id: string) => (rename.has(id) ? `url(#${rename.get(id)})` : whole));
}

export type SanitizedSvg =
  | { ok: true; svg: string; removed: string[]; namespaced: number }
  | { ok: false; error: string };

/**
 * Parse, allowlist and namespace one generated SVG. `namespace` is the block id Streamdown
 * hands to `render`; ids stay stable per block, so re-renders of the same block reuse them.
 */
/**
 * The XML parser/serializer this module needs. Injected rather than reached for globally so
 * the caller owns the environment (a test can hand in its own document), and so the module
 * can refuse to insert markup when no parser exists.
 */
export type SvgDomGlobals = { DOMParser: typeof DOMParser; XMLSerializer: typeof XMLSerializer };

export function resolveSvgDom(dom?: SvgDomGlobals): SvgDomGlobals | null {
  if (dom) return dom;
  if (typeof DOMParser === "undefined" || typeof XMLSerializer === "undefined") return null;
  return { DOMParser, XMLSerializer };
}

export function sanitizeAndNamespaceDiagramSvg(svg: string, namespace: string, dom?: SvgDomGlobals): SanitizedSvg {
  const globals = resolveSvgDom(dom);
  if (!globals) return { ok: false, error: "no XML parser available" };
  const parsed = new globals.DOMParser().parseFromString(svg, "image/svg+xml");
  const root = parsed.documentElement;
  if (!root || root.nodeName.toLowerCase() === "parsererror" || parsed.getElementsByTagName("parsererror").length > 0) {
    return { ok: false, error: "generated svg did not parse" };
  }

  const removed: string[] = [];
  const rename = new Map<string, string>();
  const prefix = `${namespace.replace(/[^\w-]/g, "") || "diagram"}-`;
  for (const element of Array.from(parsed.querySelectorAll("[id]"))) {
    const id = element.getAttribute("id");
    if (id) rename.set(id, `${prefix}${id}`);
  }

  for (const element of Array.from(parsed.querySelectorAll("*"))) {
    const name = element.nodeName.toLowerCase();
    if (!ALLOWED_TAGS.has(name)) {
      if (!removed.includes(`<${name}>`)) removed.push(`<${name}>`);
      element.remove();
      continue;
    }
    for (const attribute of Array.from(element.attributes)) {
      const attributeName = attribute.name.toLowerCase();
      if (attributeName.startsWith("on")) {
        if (!removed.includes("事件处理属性")) removed.push("事件处理属性");
        element.removeAttribute(attribute.name);
        continue;
      }
      if (attributeName === "style") {
        const withoutExternals = sanitizeCssText(attribute.value);
        const sanitizedStyle = rewriteCssFragmentIds(withoutExternals, rename);
        if (sanitizedStyle !== attribute.value) {
          const label = withoutExternals !== attribute.value ? "外部样式 URL" : "样式片段引用";
          if (!removed.includes(label)) removed.push(label);
          element.setAttribute(attribute.name, sanitizedStyle);
        }
        continue;
      }
      if (!ALLOWED_ATTRIBUTES.has(attributeName) && !attributeName.startsWith("data-") && !attributeName.startsWith("aria-")) {
        if (!removed.includes(`属性 ${attributeName}`)) removed.push(`属性 ${attributeName}`);
        element.removeAttribute(attribute.name);
        continue;
      }
      if (FRAGMENT_ATTRIBUTES.has(attributeName)) {
        // A fragment attribute is a whole-value reference: validate it, then rename it. It
        // never falls through to the generic path below.
        if (!isLocalReference(attribute.value)) {
          if (!removed.includes("外部 URL 引用")) removed.push("外部 URL 引用");
          element.removeAttribute(attribute.name);
        } else {
          const target = attribute.value.trim().slice(1);
          if (rename.has(target)) element.setAttribute(attribute.name, `#${rename.get(target)}`);
        }
        continue;
      }
      if (URL_FUNCTION_ATTRIBUTES.has(attributeName) && !isLocalUrlFunction(attribute.value) && /^url\(/i.test(attribute.value.trim())) {
        if (!removed.includes("外部 URL 引用")) removed.push("外部 URL 引用");
        element.removeAttribute(attribute.name);
        continue;
      }
      if (attributeName !== "id") {
        // Only `url(#…)` is a reference. `fill="#abc"` is a colour and is left alone even
        // when `abc` happens to be an id in this diagram.
        const rewritten = rewriteAttributeFragmentIds(attribute.value, rename);
        if (rewritten !== attribute.value) element.setAttribute(attribute.name, rewritten);
      }
    }
    const ownId = element.getAttribute("id");
    if (ownId && rename.has(ownId)) element.setAttribute("id", rename.get(ownId) as string);
    if (name === "style" && element.textContent) {
      const withoutExternals = sanitizeCssText(element.textContent);
      const sanitizedStyle = rewriteCssFragmentIds(withoutExternals, rename);
      if (sanitizedStyle !== element.textContent) {
        const label = withoutExternals !== element.textContent ? "外部样式 URL" : "样式片段引用";
        if (!removed.includes(label)) removed.push(label);
        element.textContent = sanitizedStyle;
      }
    }
  }

  return { ok: true, svg: new globals.XMLSerializer().serializeToString(root), removed, namespaced: rename.size };
}

// ---------------------------------------------------------------------------------------
// Theme + plugin
// ---------------------------------------------------------------------------------------

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
  | { ok: true; svg: string; removed: string[]; namespaced: number }
  | { ok: false; reason: "unsupported" | "too-large" | "render-failed"; error?: string; limit?: string };

export type DiagramRenderOptions = { namespace?: string; parse?: DiagramParser; renderSvg?: (source: string, options?: RenderOptions) => string; dom?: SvgDomGlobals };

/**
 * Route one fenced block. `unsupported` and in-budget `render-failed` tell the caller to use
 * the stock engine; `too-large` is terminal on purpose and must not reach another synchronous
 * layout engine.
 */
export function renderFoxDiagram(source: string, options: DiagramRenderOptions = {}): DiagramRenderResult {
  // Budget first, for every source: an oversized pie/gantt must not be routed to the stock
  // engine either, because that engine lays out synchronously too.
  const budget = checkDiagramBudget(source, options);
  if (!budget.ok) return { ok: false, reason: "too-large", limit: budget.limit };
  if (!isBeautifulMermaidSource(source)) return { ok: false, reason: "unsupported" };
  try {
    const render = options.renderSvg ?? renderMermaidSVG;
    const sanitized = sanitizeAndNamespaceDiagramSvg(render(source, diagramRenderOptions()), options.namespace ?? "diagram", options.dom);
    if (!sanitized.ok) return { ok: false, reason: "render-failed", error: sanitized.error };
    if (!sanitized.svg.trim()) return { ok: false, reason: "render-failed", error: "empty svg" };
    return { ok: true, svg: sanitized.svg, removed: sanitized.removed, namespaced: sanitized.namespaced };
  } catch (error) {
    return { ok: false, reason: "render-failed", error: error instanceof Error ? error.message : String(error) };
  }
}

/** Message shown to the user when a diagram is too large to lay out. */
export const DIAGRAM_TOO_LARGE_MESSAGE = "图表超出可渲染预算，已保留源码而未渲染，以免阻塞界面。";

export type FoxDiagramEngineOptions = {
  /** Stock engine for unsupported families and in-budget failures. */
  fallback?: DiagramPlugin;
  /** Renderer under test / future swap; still goes through budget + sanitizer. */
  renderSvg?: (source: string, options?: RenderOptions) => string;
  /** Parser used for the node/edge budget (tests inject a fake). */
  parse?: DiagramParser;
  /** XML parser/serializer; defaults to the ambient globals. */
  dom?: SvgDomGlobals;
};

/** Build the plugin; `beautifulMermaidPlugin` is the instance the app imports. */
export function createFoxMermaidPlugin(options: FoxDiagramEngineOptions = {}): DiagramPlugin {
  const fallbackSource = options.fallback ?? stockMermaid;
  return {
    name: "mermaid",
    type: "diagram",
    language: "mermaid",
    getMermaid(config): MermaidInstance {
      // The stock engine is built only when a block actually needs it. Building it up-front
      // would make every supported diagram depend on the stock engine being loadable, which
      // is exactly the coupling this adapter exists to avoid.
      let fallback: MermaidInstance | null = null;
      let pendingConfig = config;
      const engine = () => {
        if (!fallback) {
          fallback = fallbackSource.getMermaid(pendingConfig);
          if (pendingConfig) fallback.initialize(pendingConfig);
        }
        return fallback;
      };
      return {
        initialize: (next) => {
          pendingConfig = next;
          if (fallback) fallback.initialize(next);
        },
        async render(id, source) {
          const result = renderFoxDiagram(source, { namespace: id, parse: options.parse, renderSvg: options.renderSvg, dom: options.dom });
          if (result.ok) return { svg: result.svg };
          if (result.reason === "too-large") throw new Error(`${DIAGRAM_TOO_LARGE_MESSAGE}（${result.limit ?? "超出上限"}）`);
          // Unsupported family or an in-budget failure: the stock engine owns it (and its UI).
          return engine().render(id, source);
        },
      };
    },
  };
}

export const beautifulMermaidPlugin: DiagramPlugin = createFoxMermaidPlugin();
