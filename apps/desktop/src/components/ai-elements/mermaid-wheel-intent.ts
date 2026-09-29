/** Wheel zoom is opt-in so diagrams never trap ordinary conversation scrolling. */
let users = 0;
let dispose: (() => void) | undefined;

export function installMermaidWheelIntent(): () => void {
  if (++users === 1) dispose = watchWheelIntent();
  return () => { if (--users === 0) { dispose?.(); dispose = undefined; } };
}

function watchWheelIntent(): () => void {
  let active: HTMLElement | null = null;
  const cardAt = (target: EventTarget | null) => target instanceof Element
    ? target.closest<HTMLElement>('.fox-streamdown-response [data-streamdown="mermaid-block"], [data-fox-streamdown="mermaid-fullscreen"]')
    : null;
  const reset = () => { active?.removeAttribute("data-fox-wheel-active"); active = null; };
  const onPointerDown = (event: PointerEvent) => {
    const card = cardAt(event.target);
    reset();
    if (event.button !== 0 || !card || !(event.target instanceof Element)) return;
    if (!event.target.closest('[aria-label="Mermaid chart"], [role="application"]')) return;
    active = card;
    active.setAttribute("data-fox-wheel-active", "true");
  };
  const onPointerOut = (event: PointerEvent) => {
    if (active && (!(event.relatedTarget instanceof Node) || !active.contains(event.relatedTarget))) reset();
  };
  const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape") reset(); };
  const onWheel = (event: WheelEvent) => {
    const card = cardAt(event.target);
    if (!card || (card === active && active.isConnected)) return;
    // Stop the renderer's native wheel listener AND bypass the inline card's
    // scroll area. Before activation, wheel motion belongs to the conversation.
    event.stopPropagation();
    if (event.ctrlKey || event.metaKey) return;
    event.preventDefault();
    for (let parent = card.parentElement; parent; parent = parent.parentElement) {
      if (!/(auto|scroll)/.test(getComputedStyle(parent).overflowY) || parent.scrollHeight <= parent.clientHeight) continue;
      const factor = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? parent.clientHeight : 1;
      parent.scrollBy({ top: event.deltaY * factor, left: event.deltaX * factor, behavior: "instant" });
      break;
    }
  };
  document.addEventListener("pointerdown", onPointerDown, true);
  document.addEventListener("pointerout", onPointerOut, true);
  document.addEventListener("keydown", onKeyDown, true);
  document.addEventListener("wheel", onWheel, { capture: true, passive: false });
  window.addEventListener("blur", reset);
  return () => {
    reset();
    document.removeEventListener("pointerdown", onPointerDown, true);
    document.removeEventListener("pointerout", onPointerOut, true);
    document.removeEventListener("keydown", onKeyDown, true);
    document.removeEventListener("wheel", onWheel, true);
    window.removeEventListener("blur", reset);
  };
}
