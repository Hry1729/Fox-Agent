/** Keep Streamdown's existing table/diagram controls, but confine their body portals to chat. */
let users = 0;
let dispose: (() => void) | undefined;

export function installScopedStreamdownFullscreen(): () => void {
  users += 1;
  if (!dispose) dispose = watchFullscreenPortals();
  return () => {
    users -= 1;
    if (users === 0) {
      dispose?.();
      dispose = undefined;
    }
  };
}

function watchFullscreenPortals(): () => void {
  let pane: HTMLElement | null = null;
  let portal: HTMLElement | null = null;
  let trigger: HTMLButtonElement | null = null;
  let pending = false;
  const resizeObserver = new ResizeObserver(() => placePortal());

  function placePortal() {
    if (!pane?.isConnected || !portal?.isConnected) return;
    const { left, top, width, height } = pane.getBoundingClientRect();
    portal.style.setProperty("left", `${left}px`, "important");
    portal.style.setProperty("top", `${top}px`, "important");
    portal.style.setProperty("width", `${width}px`, "important");
    portal.style.setProperty("height", `${height}px`, "important");
  }

  function isFullscreenPortal(element: HTMLElement) {
    return element.dataset.streamdown === "table-fullscreen" ||
      (element.getAttribute("role") === "button" &&
        element.querySelector('button[title="Exit fullscreen"]') !== null);
  }

  function scanPortals() {
    if (portal && !portal.isConnected) {
      portal = null;
      if (!pending && trigger?.isConnected) queueMicrotask(() => trigger?.focus());
    }
    if (!pending || !pane?.isConnected) return;
    for (const child of Array.from(document.body.children)) {
      if (!(child instanceof HTMLElement) || !isFullscreenPortal(child)) continue;
      portal = child;
      portal.classList.add("fox-chat-local-fullscreen");
      // The dialog is confined to chat; controls outside that pane remain available.
      if (portal.dataset.streamdown === "table-fullscreen") {
        portal.setAttribute("aria-modal", "false");
        // Streamdown's copy/download controls locate their table through this ancestor.
        portal.dataset.foxStreamdown = "table-fullscreen";
        portal.dataset.streamdown = "table-wrapper";
      } else {
        portal.dataset.foxStreamdown = "mermaid-fullscreen";
      }
      pending = false;
      placePortal();
      break;
    }
  }

  function onClick(event: MouseEvent) {
    if (!(event.target instanceof Element)) return;
    const button = event.target.closest<HTMLButtonElement>('button[title="View fullscreen"]');
    const response = button?.closest(".fox-streamdown-response");
    const nextPane = response?.closest<HTMLElement>(".fox-chat-pane");
    if (!button || !nextPane) return;
    // The stock fullscreen button is the only control with this title.
    resizeObserver.disconnect();
    pane = nextPane;
    resizeObserver.observe(pane);
    trigger = button;
    pending = true;
    queueMicrotask(scanPortals);
  }

  const observer = new MutationObserver(scanPortals);
  observer.observe(document.body, { childList: true });
  document.addEventListener("click", onClick, true);
  window.addEventListener("resize", placePortal);
  window.addEventListener("scroll", placePortal, true);
  window.visualViewport?.addEventListener("resize", placePortal);
  window.visualViewport?.addEventListener("scroll", placePortal);

  return () => {
    observer.disconnect();
    resizeObserver.disconnect();
    document.removeEventListener("click", onClick, true);
    window.removeEventListener("resize", placePortal);
    window.removeEventListener("scroll", placePortal, true);
    window.visualViewport?.removeEventListener("resize", placePortal);
    window.visualViewport?.removeEventListener("scroll", placePortal);
  };
}
