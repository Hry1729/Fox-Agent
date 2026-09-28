"use client";

import { useState } from "react";
import type { MermaidErrorComponentProps } from "streamdown";

import "./mermaid-diagram.css";

/**
 * Shown by Streamdown when a diagram block cannot be rendered — unsupported family, invalid
 * syntax, or a source past the render budget. It keeps the answer readable: the reason is
 * stated, the original source stays available, and retry is offered. Only this block is
 * affected; the rest of the message renders as usual.
 */
export function MermaidDiagramNotice({ chart, error, retry }: MermaidErrorComponentProps) {
  const [showSource, setShowSource] = useState(false);
  return (
    <div className="fox-diagram-notice" role="alert">
      <div className="fox-diagram-notice-head">
        <strong>图表未渲染</strong>
        <div className="fox-diagram-notice-actions">
          <button
            type="button"
            aria-expanded={showSource}
            onClick={() => setShowSource((value) => !value)}
          >
            {showSource ? "隐藏源码" : "查看源码"}
          </button>
          <button type="button" onClick={retry}>重试</button>
        </div>
      </div>
      <p className="fox-diagram-notice-reason">{error}</p>
      {showSource && (
        <pre className="fox-diagram-notice-source">
          <code>{chart}</code>
        </pre>
      )}
    </div>
  );
}

export default MermaidDiagramNotice;
