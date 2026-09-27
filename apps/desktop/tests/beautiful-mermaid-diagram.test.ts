import { describe, expect, test } from "bun:test";

import {
  DIAGRAM_MAX_CHARS,
  DIAGRAM_MAX_LINES,
  beautifulMermaidPlugin,
  isBeautifulMermaidSource,
  isWithinDiagramBudget,
  renderFoxDiagram,
  sanitizeDiagramSvg,
} from "../src/components/ai-elements/beautiful-mermaid-diagram";

describe("beautiful-mermaid routing", () => {
  // Headers verified by rendering one sample each against beautiful-mermaid@1.1.3.
  const supported = [
    "flowchart TD\n  A[开始] --> B{通过?}\n  B -->|是| C[完成]",
    "graph LR\n  A --> B",
    "stateDiagram-v2\n  [*] --> 待审\n  待审 --> 通过",
    "sequenceDiagram\n  participant C as 客户端\n  C->>G: 请求",
    "classDiagram\n  class 订单\n  订单 --> 客户",
    "erDiagram\n  客户 ||--o{ 订单 : 下单",
    'xychart-beta\n  x-axis [一月, 二月]\n  bar [30, 60]',
  ];
  const unsupported = [
    'pie title 占比\n  "A" : 60',
    "gantt\n  title 排期\n  设计 :a1, 2026-01-01, 5d",
    "journey\n  title 旅程\n  第一步: 5",
    "mindmap\n  root((主题))",
  ];

  test("accepts the six implemented families", () => {
    for (const source of supported) expect(isBeautifulMermaidSource(source)).toBe(true);
  });

  test("leaves other families to the stock renderer", () => {
    for (const source of unsupported) expect(isBeautifulMermaidSource(source)).toBe(false);
  });

  test("renders a Chinese flowchart for real", () => {
    const result = renderFoxDiagram(supported[0]);
    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.svg).toContain("<svg");
    expect(result.svg).toContain("开始");
    expect(result.svg).toContain("完成");
  });

  test("pyramid of failures never throws", () => {
    for (const source of [...supported, ...unsupported, "", "flowchart TD\n  A[unterminated"]) {
      expect(() => renderFoxDiagram(source)).not.toThrow();
    }
    expect(renderFoxDiagram("pie title 占比").ok).toBe(false);
    const pie = renderFoxDiagram("pie title 占比");
    expect(pie.ok === false && pie.reason).toBe("unsupported");
  });
});

describe("diagram budget guards", () => {
  test("rejects oversized sources instead of blocking layout", () => {
    expect(isWithinDiagramBudget("flowchart TD\n  A --> B")).toBe(true);
    expect(isWithinDiagramBudget(`flowchart TD\n${"A --> B\n".repeat(DIAGRAM_MAX_LINES + 5)}`)).toBe(false);
    expect(isWithinDiagramBudget(`flowchart TD\n${"x".repeat(DIAGRAM_MAX_CHARS + 1)}`)).toBe(false);
    expect(renderFoxDiagram(`flowchart TD\n${"A[节点] --> B[节点]\n".repeat(DIAGRAM_MAX_LINES + 5)}`).ok).toBe(false);
  });
});

describe("diagram svg safety boundary", () => {
  test("strips scripts, handlers and script urls", () => {
    const dirty = '<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)"><script>alert(2)</script><a href="javascript:alert(3)">x</a><text>安全文本</text></svg>';
    const { svg, removed } = sanitizeDiagramSvg(dirty);
    expect(svg).not.toContain("<script");
    expect(svg).not.toContain("onload");
    expect(svg).not.toContain("javascript:");
    expect(svg).toContain("http://www.w3.org/2000/svg");
    expect(svg).toContain("安全文本");
    expect(removed.length).toBeGreaterThan(0);
  });

  test("leaves generated markup untouched", () => {
    const clean = renderFoxDiagram("flowchart TD\n  A[甲] --> B[乙]");
    expect(clean.ok).toBe(true);
    if (!clean.ok) return;
    expect(clean.removed).toEqual([]);
    expect(clean.svg).not.toContain("<script");
  });
});

describe("streamdown plugin contract", () => {
  test("exposes the diagram plugin shape", () => {
    expect(beautifulMermaidPlugin.name).toBe("mermaid");
    expect(beautifulMermaidPlugin.type).toBe("diagram");
    expect(beautifulMermaidPlugin.language).toBe("mermaid");
    expect(typeof beautifulMermaidPlugin.getMermaid).toBe("function");
  });
});
