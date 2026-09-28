// The real `MessageResponse` component, mounted with React DOM into a real happy-dom
// document, feeding the real Streamdown pipeline: normal markdown and code blocks must stay
// intact, a Mermaid block must render through the beautiful-mermaid plugin, a diagram that
// cannot render must not take the rest of the answer with it, and a remount (the history
// reopen analog) must render again.
//
// SCOPE (honest): the Tauri host, the live model and a packaged-app click-through are NOT
// part of this file. "The model chose to draw" is a separate manual acceptance item.

import { describe, expect, test } from "bun:test";
import { GlobalRegistrator } from "@happy-dom/global-registrator";

try {
  GlobalRegistrator.register();
} catch {
  // Another test file in this process already registered happy-dom.
}
(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

import { MessageResponse } from "../src/components/ai-elements/message-response";

const FLOWCHART = "```mermaid\nflowchart TD\n  A[开始] --> B{通过?}\n  B -->|是| C[完成]\n```";
const UNSUPPORTED = '```mermaid\npie title 占比\n  "A" : 60\n```';

async function mount(markdown: string, options: { expectDiagram?: boolean } = {}) {
  const container = document.createElement("div");
  document.body.appendChild(container);
  let root: Root | null = createRoot(container);
  await act(async () => {
    root?.render(<MessageResponse>{markdown}</MessageResponse>);
  });
  const deadline = Date.now() + (options.expectDiagram ? 10_000 : 2_500);
  while (Date.now() < deadline && !container.querySelector("marker")) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 50));
    });
  }
  return {
    container,
    text: () => container.textContent ?? "",
    html: () => container.innerHTML,
    unmount: async () => {
      await act(async () => {
        root?.unmount();
      });
      root = null;
      container.remove();
    },
  };
}

describe("message component with diagrams", () => {
  test("keeps normal markdown and code blocks unchanged", async () => {
    const mounted = await mount("# 标题\n\n段落文本\n\n```ts\nconst a = 1\n```\n");
    const text = mounted.text();
    expect(text).toContain("标题");
    expect(text).toContain("段落文本");
    expect(mounted.container.querySelector("pre")?.textContent ?? "").toContain("const a = 1");
    // `<svg>` alone is not a diagram signal — lucide icons are svg too — so assert on an
    // element only the diagram renderer emits.
    expect(mounted.container.querySelector("marker")).toBeNull();
    await mounted.unmount();
  });

  // SKIPPED, with the reason rather than a passing assertion: Streamdown's diagram path does
  // not produce a diagram inside happy-dom (it needs a real browser layout/observer surface),
  // so this cannot be evidence here. The engine itself is covered by the plugin-level tests in
  // beautiful-mermaid-diagram.test.ts, which call the real render path; proving the diagram
  // appears in an answer stays a packaged-app / manual acceptance item, and is reported as
  // unverified rather than papered over.
  test.skip("renders a mermaid block through the plugin and keeps the prose", async () => {
    const mounted = await mount(`前一段说明文字。\n\n${FLOWCHART}\n\n图后解释文字。\n`, { expectDiagram: true });
    expect(mounted.text()).toContain("前一段说明文字");
    expect(mounted.text()).toContain("图后解释文字");
    const svg = mounted.container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(mounted.container.querySelector("marker")).not.toBeNull();
    expect(mounted.html()).toContain("开始");
    // Diagram ids are namespaced per block, so several diagrams cannot collide.
    const ids = Array.from(mounted.container.querySelectorAll("[id]")).map((element) => element.id);
    expect(ids.length).toBeGreaterThan(0);
    expect(new Set(ids).size).toBe(ids.length);
    await mounted.unmount();
    // Streamdown resolves the diagram engine asynchronously; bun's 5s default is too tight.
  }, 25_000);

  test("a diagram that cannot render does not remove the rest of the answer", async () => {
    const mounted = await mount(`前面的话。\n\n${UNSUPPORTED}\n\n后面的话。\n`);
    expect(mounted.text()).toContain("前面的话");
    expect(mounted.text()).toContain("后面的话");
    await mounted.unmount();
  });

  // Skipped for the same environment reason as above.
  test.skip("a remount renders the same diagram again", async () => {
    const first = await mount(FLOWCHART, { expectDiagram: true });
    const firstIds = Array.from(first.container.querySelectorAll("[id]")).map((element) => element.id).sort();
    await first.unmount();

    const second = await mount(FLOWCHART, { expectDiagram: true });
    const secondIds = Array.from(second.container.querySelectorAll("[id]")).map((element) => element.id).sort();
    expect(second.container.querySelector("marker")).not.toBeNull();
    // Stable ids per block keep a re-render from being treated as a new diagram.
    expect(secondIds).toEqual(firstIds);
    await second.unmount();
  }, 25_000);
});
