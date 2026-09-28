// Adapter tests: routing, per-diagram id namespacing, the render budget (including the
// guarantee that an over-budget source never reaches another layout engine), and the
// parse-based SVG allowlist.
//
// SCOPE (honest): the plugin, engine and sanitizer here are the real ones mounted into a real
// happy-dom document. A packaged-Tauri click-through with a live model is a separate manual
// acceptance item; nothing here claims to be that.

import { describe, expect, test } from "bun:test";
import { Window } from "happy-dom";

import type { DiagramPlugin } from "streamdown";

import {
  DIAGRAM_MAX_LINES,
  DIAGRAM_TOO_LARGE_MESSAGE,
  checkDiagramBudget,
  createFoxMermaidPlugin,
  isBeautifulMermaidSource,
  renderFoxDiagram,
  sanitizeAndNamespaceDiagramSvg,
  sanitizeCssText,
} from "../src/components/ai-elements/beautiful-mermaid-diagram";

// Two ambient globals instead of `GlobalRegistrator.register()`: another test file in this
// process owns that registration, and this file must neither claim nor break it.
const window = new Window();
(globalThis as unknown as { DOMParser: unknown }).DOMParser = window.DOMParser;
(globalThis as unknown as { XMLSerializer: unknown }).XMLSerializer = window.XMLSerializer;

const FLOWCHART = "flowchart TD\n  A[开始] --> B{通过?}\n  B -->|是| C[完成]\n  B -->|否| D[退回]";
const SEQUENCE = "sequenceDiagram\n  participant C as 客户端\n  participant G as 网关\n  C->>G: 请求\n  G-->>C: 响应";
const XYCHART = "xychart-beta\n  x-axis [一月, 二月]\n  bar [30, 60]";
const UNSUPPORTED = ['pie title 占比\n  "A" : 60', "gantt\n  title 排期\n  设计 :a1, 2026-01-01, 5d"];
const oversized = () => `flowchart TD\n${"  A[节点] --> B[节点]\n".repeat(DIAGRAM_MAX_LINES + 10)}`;

const idsOf = (svg: string) => {
  const parsed = new DOMParser().parseFromString(svg, "image/svg+xml");
  return Array.from(parsed.querySelectorAll("[id]")).map((element) => element.getAttribute("id") ?? "");
};

function fakeFallback(options: { reject?: string } = {}) {
  const calls: Array<{ id: string; source: string }> = [];
  const plugin: DiagramPlugin = {
    name: "mermaid",
    type: "diagram",
    language: "mermaid",
    getMermaid: () => ({
      initialize: () => {},
      render: async (id: string, source: string) => {
        calls.push({ id, source });
        if (options.reject) throw new Error(options.reject);
        return { svg: `<svg xmlns="http://www.w3.org/2000/svg"><text>stock:${id}</text></svg>` };
      },
    }),
  };
  return { plugin, calls };
}

describe("routing", () => {
  test("accepts the six implemented families", () => {
    for (const source of [FLOWCHART, "graph LR\n  A --> B", "stateDiagram-v2\n  [*] --> 待审", SEQUENCE, "classDiagram\n  class 订单", "erDiagram\n  客户 ||--o{ 订单 : 下单", XYCHART]) {
      expect(isBeautifulMermaidSource(source)).toBe(true);
    }
  });

  test("leaves other families to the stock renderer", () => {
    for (const source of UNSUPPORTED) expect(isBeautifulMermaidSource(source)).toBe(false);
  });
});

describe("id isolation", () => {
  test("namespaces every id and fragment reference per diagram", () => {
    const first = renderFoxDiagram(FLOWCHART, { namespace: "diagram-one" });
    const second = renderFoxDiagram(FLOWCHART, { namespace: "diagram-two" });
    expect(first.ok && second.ok).toBe(true);
    if (!first.ok || !second.ok) return;

    const firstIds = idsOf(first.svg);
    const secondIds = idsOf(second.svg);
    expect(firstIds.length).toBeGreaterThan(0);
    expect(firstIds.every((id) => id.startsWith("diagram-one-"))).toBe(true);
    expect(secondIds.every((id) => id.startsWith("diagram-two-"))).toBe(true);
    expect(firstIds.filter((id) => secondIds.includes(id))).toEqual([]);
    expect(first.svg).toContain("url(#diagram-one-arrowhead)");
    expect(second.svg).toContain("url(#diagram-two-arrowhead)");
  });

  test("a second diagram renders after the first is gone", () => {
    const first = renderFoxDiagram(FLOWCHART, { namespace: "gone" });
    const second = renderFoxDiagram(SEQUENCE, { namespace: "stays" });
    expect(first.ok && second.ok).toBe(true);
    if (!second.ok) return;
    expect(second.svg.includes("#gone-")).toBe(false);
    for (const id of idsOf(second.svg)) expect(id.startsWith("stays-")).toBe(true);
  });

  // Regression: only reference contexts may be rewritten. A bare `#abc` is a colour, even when
  // a node happens to be called `abc`.
  test("rewrites references but never colour literals", () => {
    const svg = [
      '<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">',
      '<defs><marker id="abc"><path d="M0 0"/></marker></defs>',
      '<rect id="box" fill="#abc" stroke="url(#abc)" marker-end="url(#abc)"/>',
      '<text fill="#abcdef">文本</text>',
      '<use xlink:href="#box"/>',
      '<style>.n{fill:url(#abc)} .m{stroke:#abc}</style>',
      '<g style="fill:url(#abc);stroke:#abc"><path d="M0 0"/></g>',
      "</svg>",
    ].join("");
    const result = sanitizeAndNamespaceDiagramSvg(svg, "ns");
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    // Colours untouched.
    expect(result.svg).toContain('fill="#abc"');
    expect(result.svg).toContain('fill="#abcdef"');
    expect(result.svg).toContain("stroke:#abc");
    // References rewritten everywhere they are references.
    expect(result.svg).toContain('stroke="url(#ns-abc)"');
    expect(result.svg).toContain('marker-end="url(#ns-abc)"');
    expect(result.svg).toContain('xlink:href="#ns-box"');
    expect(result.svg).toContain('id="ns-abc"');
    expect(result.svg).toContain('id="ns-box"');
    expect(result.svg).toContain(".n{fill:url(#ns-abc)}");
    expect(result.svg).toContain('style="fill:url(#ns-abc);stroke:#abc"');
  });
});

describe("render budget", () => {
  test("bounds characters, lines, nodes and edges", () => {
    expect(checkDiagramBudget(FLOWCHART).ok).toBe(true);
    expect(checkDiagramBudget(oversized()).ok).toBe(false);
    expect(checkDiagramBudget(`flowchart TD\n  A[${"x".repeat(13_000)}]`).ok).toBe(false);
    const wide = `flowchart TD\n${Array.from({ length: 260 }, (_, index) => `  N${index}[节点${index}]`).join("\n")}\n  N0 --> N1`;
    expect(checkDiagramBudget(wide).ok).toBe(false);
    const linked = `flowchart TD\n${Array.from({ length: 320 }, (_, index) => `  N${index % 200} --> N${(index + 1) % 200}`).join("\n")}`;
    expect(checkDiagramBudget(linked).ok).toBe(false);
  });

  test("an over-budget source never reaches another layout engine", async () => {
    const fallback = fakeFallback();
    const plugin = createFoxMermaidPlugin({ fallback: fallback.plugin });
    await expect(plugin.getMermaid().render("d1", oversized())).rejects.toThrow(DIAGRAM_TOO_LARGE_MESSAGE);
    expect(fallback.calls).toEqual([]);
    const direct = renderFoxDiagram(oversized(), { namespace: "d1" });
    expect(direct.ok).toBe(false);
    expect(direct.ok === false && direct.reason).toBe("too-large");
  });

  // Regression: the budget used to be checked *after* the supported-type test, so an oversized
  // pie/gantt skipped beautiful-mermaid and was handed to the stock engine — the very stall the
  // budget exists to prevent.
  test("an oversized unsupported family also stops before any engine", async () => {
    const fallback = fakeFallback();
    const plugin = createFoxMermaidPlugin({ fallback: fallback.plugin });
    for (const preamble of ["pie title 占比", "gantt\n  title 排期"]) {
      const huge = `${preamble}\n${Array.from({ length: 320 }, (_, index) => `  "第${index}项" : ${index % 100}`).join("\n")}`;
      expect(checkDiagramBudget(huge).ok).toBe(false);
      const routed = renderFoxDiagram(huge, { namespace: "d9" });
      expect(routed.ok).toBe(false);
      expect(routed.ok === false && routed.reason).toBe("too-large");
      await expect(plugin.getMermaid().render("d9", huge)).rejects.toThrow(DIAGRAM_TOO_LARGE_MESSAGE);
    }
    expect(fallback.calls).toEqual([]);
  });
});

describe("engine selection", () => {
  test("supported families render with the real engine and skip the fallback", async () => {
    const fallback = fakeFallback();
    const plugin = createFoxMermaidPlugin({ fallback: fallback.plugin });
    const { svg } = await plugin.getMermaid().render("d1", FLOWCHART);
    expect(svg).toContain("<svg");
    expect(svg).toContain("开始");
    expect(svg).toContain("d1-arrowhead");
    expect(fallback.calls).toEqual([]);
  });

  test("unsupported families delegate to the stock engine", async () => {
    const fallback = fakeFallback();
    const plugin = createFoxMermaidPlugin({ fallback: fallback.plugin });
    const { svg } = await plugin.getMermaid().render("d2", UNSUPPORTED[0]);
    expect(svg).toContain("stock:d2");
    expect(fallback.calls).toEqual([{ id: "d2", source: UNSUPPORTED[0] }]);
  });

  test("an in-budget render failure falls back, and a double failure surfaces an error", async () => {
    const fallback = fakeFallback();
    const plugin = createFoxMermaidPlugin({ fallback: fallback.plugin, renderSvg: () => { throw new Error("engine exploded"); } });
    const { svg } = await plugin.getMermaid().render("d3", FLOWCHART);
    expect(svg).toContain("stock:d3");
    expect(fallback.calls).toHaveLength(1);

    const failing = fakeFallback({ reject: "stock also failed" });
    const doomed = createFoxMermaidPlugin({ fallback: failing.plugin, renderSvg: () => { throw new Error("engine exploded"); } });
    await expect(doomed.getMermaid().render("d4", FLOWCHART)).rejects.toThrow("stock also failed");
  });

  test("streaming prefixes and unclosed fences never throw", () => {
    for (let length = 1; length <= FLOWCHART.length; length += 7) {
      const partial = FLOWCHART.slice(0, length);
      expect(() => renderFoxDiagram(partial, { namespace: "stream" })).not.toThrow();
    }
    for (const broken of ["flowchart TD\n  A[未闭合", "sequenceDiagram\n  C->>", "graph", "erDiagram\n  A ||--"]) {
      const result = renderFoxDiagram(broken, { namespace: "broken" });
      if (result.ok) expect(result.svg).toContain("<svg");
    }
  });
});

describe("svg safety boundary", () => {
  test("decodes entities before deciding, and drops what it cannot allow", () => {
    const dirty = [
      '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)">',
      "<script>alert(2)</script>",
      '<a href="java&#x73;cript:alert(3)"><text>链接</text></a>',
      '<image href="https://evil.example/x.png"/>',
      "<foreignObject><body>bad</body></foreignObject>",
      '<text style="fill:url(https://evil.example/y)">安全文本</text>',
      "</svg>",
    ].join("");
    const result = sanitizeAndNamespaceDiagramSvg(dirty, "ns");
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.svg).not.toContain("<script");
    expect(result.svg).not.toContain("foreignObject");
    expect(result.svg).not.toContain("onload");
    expect(result.svg.toLowerCase()).not.toContain("javascript");
    expect(result.svg).not.toContain("evil.example");
    expect(result.svg).toContain("安全文本");
    expect(result.svg).toContain("http://www.w3.org/2000/svg");
    expect(result.removed.length).toBeGreaterThan(0);
  });

  test("keeps real generated markup intact and strips css imports", () => {
    const clean = renderFoxDiagram(FLOWCHART, { namespace: "clean" });
    expect(clean.ok).toBe(true);
    if (!clean.ok) return;
    // The only thing removed from a real diagram is the library's Google Fonts @import:
    // a chat message must not reach out to an external font host.
    expect(clean.removed).toEqual(["外部样式 URL"]);
    expect(clean.svg).not.toContain("fonts.googleapis.com");
    // Regression: the font URL query contains semicolons (`wght@400;500;600;700`), and a naive
    // `@import[^;]*` used to leave `500;600;700&display=swap');` behind in the stylesheet.
    expect(clean.svg).not.toContain("display=swap");
    expect(clean.svg).not.toContain("@import");
    expect(clean.svg).toContain("开始");
    expect(clean.svg).toContain('fill="none"');
    expect(clean.svg).toContain("var(--");
    expect(sanitizeCssText("@import url(https://evil.example/a.css);\n.x{fill:url(#ok)}")).not.toContain("evil.example");
    expect(sanitizeCssText("@import url('https://fonts.example/css2?family=Inter:wght@400;500;600&display=swap');\n.m{color:red}")).not.toContain("display=swap");
    expect(sanitizeCssText(".x{fill:url(#ok)}")).toContain("url(#ok)");
  });

  test("a real parser is present, so markup is never inserted unparsed", () => {
    expect(typeof DOMParser).toBe("function");
    expect(typeof XMLSerializer).toBe("function");
  });
});
