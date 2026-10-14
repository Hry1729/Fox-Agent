// Source: https://uiarc.dev/r/shortcut-recorder.json
"use client";

import { forwardRef, useCallback, useEffect, useId, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { HTMLAttributes, KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { RotateCcw, Search, X } from "lucide-react";
import { motionTokens } from "../lib/motion-tokens";
import styles from "./shortcut-recorder.module.css";

// Fox uses the same pure primitives for recording, conflict detection, and app-wide matching.
import { detectShortcutPlatform, formatShortcut, keyId, normalizeShortcut, shortcutFromEvent, shortcutTokens, splitShortcut } from "@/features/settings/shortcut-model";
import type { Platform } from "@/features/settings/shortcut-model";
export { formatShortcut, keyId, matchesShortcut, normalizeShortcut, shortcutFromEvent, shortcutTokens } from "@/features/settings/shortcut-model";
export type { Platform, ShortcutToken } from "@/features/settings/shortcut-model";

const subscribe = () => () => {};
/** Reduced motion that only applies after hydration, so server and client render the same styles. */
function useReducedFlag() {
  const hydrated = useSyncExternalStore(subscribe, () => true, () => false);
  return !!useReducedMotion() && hydrated;
}
/** The viewer's platform. Server renders assume a non-Apple platform and settle on the first client render. */
export function usePlatform(override?: Platform): Platform {
  const detected = useSyncExternalStore(subscribe, detectShortcutPlatform, () => "other" as Platform);
  return override ?? detected;
}

/** Keys held down right now, by physical id. Clears on window blur, and after Command lifts (macOS drops the other keyups). */
export function usePressedKeys(enabled = true) {
  const [held, setHeld] = useState<ReadonlySet<string>>(() => new Set());
  useEffect(() => {
    if (!enabled) return;
    const update = (change: (next: Set<string>) => void) => setHeld(current => { const next = new Set(current); change(next); return next.size === current.size && [...next].every(key => current.has(key)) ? current : next; });
    const down = (event: KeyboardEvent) => { const id = keyId(event); if (id) update(next => next.add(id)); };
    const up = (event: KeyboardEvent) => {
      const id = keyId(event);
      update(next => {
        if (id) next.delete(id);
        if (id === "meta") [...next].forEach(key => { if (!["ctrl", "alt", "shift"].includes(key)) next.delete(key); });
        if (!event.metaKey) next.delete("meta"); if (!event.ctrlKey) next.delete("ctrl"); if (!event.altKey) next.delete("alt"); if (!event.shiftKey) next.delete("shift");
      });
    };
    const clear = () => setHeld(current => current.size ? new Set() : current);
    // Capture phase, so keys still register while a focused recorder keeps the chord from bubbling.
    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up, true);
    window.addEventListener("blur", clear);
    document.addEventListener("visibilitychange", clear);
    return () => { window.removeEventListener("keydown", down, true); window.removeEventListener("keyup", up, true); window.removeEventListener("blur", clear); document.removeEventListener("visibilitychange", clear); };
  }, [enabled]);
  return enabled ? held : EMPTY;
}
const EMPTY: ReadonlySet<string> = new Set();

/* ================================================================================================
 * ShortcutKbd and ShortcutKeys
 * ============================================================================================== */

export interface ShortcutKbdProps extends HTMLAttributes<HTMLElement> {
  children: ReactNode;
  /** Draws the key pushed down and lit, such as while it is held. */
  pressed?: boolean;
  size?: "sm" | "md";
}

/** A key cap. Use it inline in copy, in menus, or through `ShortcutKeys` for a whole combination. */
export const ShortcutKbd = forwardRef<HTMLElement, ShortcutKbdProps>(function ShortcutKbd({ children, pressed = false, size = "md", className, ...props }, ref) {
  return <kbd ref={ref} {...props} className={[styles.kbd, className].filter(Boolean).join(" ")} data-size={size} data-pressed={pressed || undefined}>{children}</kbd>;
});
ShortcutKbd.displayName = "ShortcutKbd";
/** @deprecated Renamed to `ShortcutKbd` so it never clashes with the `kbd` component's `Kbd` in a shared index.ts. */
export const Kbd = ShortcutKbd;
/** @deprecated Renamed to `ShortcutKbdProps`. */
export type KbdProps = ShortcutKbdProps;

export interface ShortcutKeysProps {
  shortcut: string;
  platform?: Platform;
  /** Held key ids from `usePressedKeys`; matching caps light up. */
  pressed?: ReadonlySet<string>;
  size?: "sm" | "md";
  className?: string;
}

/** A shortcut as key caps, with a spoken label for assistive technology. */
export function ShortcutKeys({ shortcut, platform: platformProp, pressed, size = "md", className }: ShortcutKeysProps) {
  const platform = usePlatform(platformProp);
  const tokens = shortcutTokens(shortcut, platform);
  return <span className={[styles.keys, className].filter(Boolean).join(" ")} role="img" aria-label={tokens.map(token => token.spoken).join(" ")}>
    {tokens.map((token, index) => <ShortcutKbd key={`${index}-${token.id}`} size={size} pressed={pressed?.has(token.id)} aria-hidden="true">{token.label}</ShortcutKbd>)}
  </span>;
}

/* ================================================================================================
 * ShortcutRecorder
 * ============================================================================================== */

export interface ShortcutBinding { shortcut: string; label: string }

export interface ShortcutRecorderProps {
  label: string;
  hideLabel?: boolean;
  value?: string | null;
  defaultValue?: string | null;
  /** `replaced` is the binding the new shortcut was taken from after "Use anyway", so the caller can unbind it. */
  onValueChange?: (value: string | null, details: { replaced?: ShortcutBinding }) => void | boolean;
  /** What the reset button restores. Defaults to `defaultValue`. */
  resetValue?: string | null;
  /** Shortcuts already in use. Recording one of them asks before taking it. */
  bindings?: ShortcutBinding[];
  /** Also warn about combinations the browser or system keeps, such as ⌘W and ⌘C. On by default. */
  warnReserved?: boolean;
  /** Require ⌘, Ctrl, or ⌥ (function keys are always allowed). On by default. */
  requireModifier?: boolean;
  platform?: Platform;
  placeholder?: string;
  description?: string;
  disabled?: boolean;
  id?: string;
  className?: string;
}

const RESERVED: ShortcutBinding[] = [
  { shortcut: "mod+w", label: "关闭标签页" }, { shortcut: "mod+t", label: "新建标签页" }, { shortcut: "mod+n", label: "新建窗口" }, { shortcut: "mod+q", label: "退出应用" },
  { shortcut: "mod+l", label: "地址栏" }, { shortcut: "mod+r", label: "重新加载" }, { shortcut: "mod+shift+t", label: "重新打开标签页" },
  { shortcut: "mod+c", label: "复制" }, { shortcut: "mod+v", label: "粘贴" }, { shortcut: "mod+x", label: "剪切" }, { shortcut: "mod+z", label: "撤销" }, { shortcut: "mod+a", label: "全选" },
];

type Pending = { shortcut: string; conflict: ShortcutBinding; reserved: boolean };

/**
 * A field that records a keyboard shortcut. Press it, then press the combination: held modifiers light up as caps while you
 * build the chord, the finished shortcut settles into place, and a combination that is already taken asks before it is used.
 * Escape cancels, Backspace clears, and the reset button restores the default. Caps follow the platform: ⌘ on a Mac, Ctrl elsewhere.
 */
export function ShortcutRecorder({
  label, hideLabel = false, value, defaultValue = null, onValueChange, resetValue, bindings = [], warnReserved = true, requireModifier = true,
  platform: platformProp, placeholder = "录制快捷键", description, disabled = false, id, className,
}: ShortcutRecorderProps) {
  const reduced = useReducedFlag();
  const platform = usePlatform(platformProp);
  const uid = useId();
  const buttonId = id ?? `${uid}-button`;
  const labelId = `${uid}-label`, messageId = `${uid}-message`;

  const [inner, setInner] = useState<string | null>(defaultValue);
  const current = value !== undefined ? value : inner;
  const restoreTo = resetValue !== undefined ? resetValue : defaultValue;

  const [recording, setRecording] = useState(false);
  const [live, setLive] = useState<string[]>([]);
  const [pending, setPending] = useState<Pending | null>(null);
  const [nudge, setNudge] = useState<string | null>(null);
  const [settled, setSettled] = useState(0);
  const [announcement, setAnnouncement] = useState("");
  const buttonRef = useRef<HTMLButtonElement>(null);
  /* The message slot animates to the measured height of its content, so a swap to a message that wraps grows instead of snapping. */
  const [messageHeight, setMessageHeight] = useState<number | null>(null);
  const measureMessage = useCallback((node: HTMLDivElement | null) => {
    if (!node || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => setMessageHeight(node.getBoundingClientRect().height));
    observer.observe(node);
    return () => { observer.disconnect(); setMessageHeight(null); };
  }, []);

  const spoken = (shortcut: string | null) => shortcut ? formatShortcut(shortcut, platform).spoken : "未设置";

  const commit = useCallback((next: string | null, replaced?: ShortcutBinding) => {
    setPending(null);
    setNudge(null);
    if (onValueChange?.(next, { replaced }) === false) {
      setAnnouncement("快捷键未保存，请重试");
      return;
    }
    if (value === undefined) setInner(next);
    setSettled(count => count + 1);
    setAnnouncement(next ? `快捷键已设置为 ${formatShortcut(next, platform).spoken}` : "快捷键已清除");
  }, [onValueChange, platform, value]);

  const start = () => {
    if (disabled) return;
    setPending(null); setNudge(null); setLive([]);
    setRecording(true);
    setAnnouncement("正在录制。按下新的组合键，或按 Esc 取消。");
  };
  const stop = () => { setRecording(false); setLive([]); };

  const conflictFor = (shortcut: string): Pending | null => {
    const normal = normalizeShortcut(shortcut, platform);
    if (current && normalizeShortcut(current, platform) === normal) return null;
    const taken = bindings.find(binding => normalizeShortcut(binding.shortcut, platform) === normal);
    if (taken) return { shortcut, conflict: taken, reserved: false };
    const reserved = warnReserved ? RESERVED.find(binding => normalizeShortcut(binding.shortcut, platform) === normal) : undefined;
    return reserved ? { shortcut, conflict: reserved, reserved: true } : null;
  };

  const liveMods = (event: ReactKeyboardEvent) => {
    const mods: string[] = [];
    if (event.ctrlKey && platform === "mac") mods.push("ctrl");
    if (event.altKey) mods.push("alt");
    if (event.shiftKey) mods.push("shift");
    if (platform === "mac" ? event.metaKey : event.ctrlKey) mods.push("mod");
    if (platform === "other" && event.metaKey) mods.push("meta");
    return mods;
  };

  function onKeyDown(event: ReactKeyboardEvent<HTMLButtonElement>) {
    if (!recording) {
      if ((event.key === "Backspace" || event.key === "Delete") && current) { event.preventDefault(); commit(null); }
      return;
    }
    if (event.key === "Tab" && !event.metaKey && !event.ctrlKey && !event.altKey) { stop(); return; }
    // The chord belongs to the recorder: keep it from the page's own shortcuts, such as a ⌘K command menu.
    event.stopPropagation();
    if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229 || event.repeat) return;
    event.preventDefault();
    const bare = !event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey;
    if (bare && event.key === "Escape") { stop(); setAnnouncement("已取消录制"); return; }
    if (bare && (event.key === "Backspace" || event.key === "Delete")) { stop(); commit(null); return; }
    const shortcut = shortcutFromEvent(event.nativeEvent, platform);
    if (!shortcut) { setLive(liveMods(event)); return; }
    const { mods, key } = splitShortcut(shortcut);
    const strong = mods.has("mod") || mods.has("ctrl") || mods.has("alt") || mods.has("meta");
    if (requireModifier && !strong && !/^f\d+$/.test(key)) {
      const hint = platform === "mac" ? "请同时按下 ⌘、⌃ 或 ⌥" : "请同时按下 Ctrl 或 Alt";
      setNudge(hint); setAnnouncement(hint); setLive(liveMods(event));
      return;
    }
    stop();
    const conflict = conflictFor(shortcut);
    if (conflict) {
      setPending(conflict);
      setAnnouncement(`${formatShortcut(shortcut, platform).spoken} 已用于 ${conflict.conflict.label}。可确认使用，或重新录制。`);
      return;
    }
    commit(shortcut);
  }
  function onKeyUp(event: ReactKeyboardEvent<HTMLButtonElement>) { if (recording) setLive(liveMods(event)); }

  const shown = pending?.shortcut ?? current;
  const tokens = recording ? shortcutTokens(live.join("+"), platform) : shown ? shortcutTokens(shown, platform) : [];
  const state = recording ? "recording" : pending ? "conflict" : "idle";
  const canReset = !recording && (pending !== null || (current ?? null) !== (restoreTo ?? null));
  const canClear = !recording && !!current && !pending;
  const message = pending
    ? { tone: "warning" as const, key: `c-${pending.shortcut}`, body: <>{pending.reserved ? "系统通常用于" : "已用于"} <strong className={styles.strong}>{pending.conflict.label}</strong></> }
    : nudge ? { tone: "hint" as const, key: `n-${nudge}`, body: <>{nudge}</> }
    : recording ? { tone: "hint" as const, key: "rec", body: <>Esc 取消，Backspace 清除</> }
    : description ? { tone: "hint" as const, key: "d", body: <>{description}</> } : null;

  // A reset or storage update in another window must dismiss an obsolete recording/conflict.
  useEffect(() => {
    setPending(null);
    setNudge(null);
    setRecording(false);
    setLive([]);
  }, [current]);

  useEffect(() => {
    if (!settled) return;
    const timer = window.setTimeout(() => setSettled(0), 700);
    return () => window.clearTimeout(timer);
  }, [settled]);

  return <div className={[styles.field, className].filter(Boolean).join(" ")} data-disabled={disabled || undefined} data-shortcut-recorder="" data-recording={recording || undefined}>
    <span id={labelId} className={hideLabel ? styles.srOnly : styles.label}>{label}</span>
    <div className={styles.control} data-state={state}>
      <button ref={buttonRef} id={buttonId} type="button" className={styles.recorder} disabled={disabled} aria-labelledby={`${labelId} ${buttonId}`} aria-describedby={message ? messageId : undefined}
        aria-pressed={recording}
        onClick={event => { if (recording) { if (event.detail !== 0) { stop(); setAnnouncement("已取消录制"); } return; } start(); }}
        onKeyDown={onKeyDown} onKeyUp={onKeyUp} onBlur={() => { if (recording) { stop(); setNudge(null); } }}>
        <span className={styles.srOnly}>{recording ? "正在录制" : shown ? spoken(shown) : placeholder}</span>
        <span className={styles.row} aria-hidden="true">
          {recording && <span className={styles.dot} data-reduced={reduced || undefined} />}
          {/* Chords change faster than exit/layout springs settle. Keep one live
              set of caps in normal flow, including cancel, commit and blur. */}
          {tokens.map(token => <span key={token.id} className={styles.chipSlot}>
            <ShortcutKbd pressed={recording || (settled > 0 && !pending)} data-tone={pending ? "warning" : undefined}>{token.label}</ShortcutKbd>
          </span>)}
          {tokens.length === 0 && <span className={styles.placeholder} data-recording={recording || undefined}>
            {recording ? "按下组合键" : placeholder}
          </span>}
        </span>
      </button>

      <span className={styles.actions}>
        <AnimatePresence initial={false}>
          {canReset && <motion.button key="reset" type="button" className={styles.action} aria-label={`恢复默认：${restoreTo ? formatShortcut(restoreTo, platform).spoken : "未设置"}`}
            onClick={() => { const conflict = restoreTo ? conflictFor(restoreTo) : null; if (conflict) setPending(conflict); else commit(restoreTo ?? null); buttonRef.current?.focus(); }}
            initial={{ opacity: 0, scale: .6 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: .6, transition: { duration: .1 } }} transition={reduced ? { duration: 0 } : motionTokens.spring.snappy}>
            <RotateCcw size={16} strokeWidth={1.75} aria-hidden="true" />
          </motion.button>}
          {canClear && <motion.button key="clear" type="button" className={styles.action} aria-label="清除快捷键" onClick={() => { commit(null); buttonRef.current?.focus(); }}
            initial={{ opacity: 0, scale: .6 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: .6, transition: { duration: .1 } }} transition={reduced ? { duration: 0 } : motionTokens.spring.snappy}>
            <X size={16} strokeWidth={1.75} aria-hidden="true" />
          </motion.button>}
        </AnimatePresence>
      </span>
    </div>

    <AnimatePresence initial={false}>
      {message && <motion.div key="slot" className={styles.messageSlot} initial={reduced ? false : { height: 0, opacity: 0 }} animate={{ height: messageHeight ?? "auto", opacity: 1 }}
        exit={{ height: 0, opacity: 0, transition: reduced ? { duration: 0 } : { height: motionTokens.spring.smooth, opacity: { duration: .1 } } }}
        transition={reduced ? { duration: 0 } : { height: motionTokens.spring.smooth, opacity: { duration: motionTokens.duration.fast } }}>
        <div ref={measureMessage}>
          <AnimatePresence initial={false} mode="popLayout">
            <motion.div key={message.key} id={messageId} className={styles.message} data-tone={message.tone}
              initial={reduced ? { opacity: 0 } : { opacity: 0, y: "0.35em", filter: `blur(${motionTokens.blur.soft}px)` }} animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
              exit={{ opacity: 0, transition: { duration: .08 } }} transition={{ duration: reduced ? .1 : motionTokens.duration.standard, ease: [...motionTokens.ease.enter] }}>
              <span className={styles.messageText}>{message.body}</span>
              {pending && <span className={styles.messageActions}>
                <button type="button" className={styles.link} onClick={() => { const taken = pending; commit(taken.shortcut, taken.reserved ? undefined : taken.conflict); buttonRef.current?.focus(); }}>{pending.reserved ? "仍然使用" : "替换原快捷键"}</button>
                <button type="button" className={styles.link} data-quiet="" onClick={() => { setPending(null); setAnnouncement("保留原快捷键"); buttonRef.current?.focus(); }}>取消</button>
              </span>}
            </motion.div>
          </AnimatePresence>
        </div>
      </motion.div>}
    </AnimatePresence>
    <span className={styles.srOnly} role="status" aria-live="polite">{announcement}</span>
  </div>;
}

/* ================================================================================================
 * ShortcutList
 * ============================================================================================== */

export interface ShortcutListItem { label: string; shortcut: string; /** Extra words that match when searching. */ keywords?: string }
export interface ShortcutListGroup { label: string; items: ShortcutListItem[] }

export interface ShortcutListProps {
  groups: ShortcutListGroup[];
  /** Accessible name of the list. */
  label?: string;
  searchable?: boolean;
  searchPlaceholder?: string;
  /** Light up caps as keys are held, and fade rows that do not use the held modifiers. On by default. */
  highlightPressed?: boolean;
  platform?: Platform;
  className?: string;
}

/**
 * A searchable, grouped cheatsheet. Holding a modifier lights that cap in every row and fades rows that do not use it, so people
 * can explore by holding keys; a row whose whole combination is down lights up. Search matches names, groups, and key names such as "cmd k".
 */
export function ShortcutList({ groups, label = "快捷键", searchable = true, searchPlaceholder = "搜索快捷键", highlightPressed = true, platform: platformProp, className }: ShortcutListProps) {
  const reduced = useReducedFlag();
  const platform = usePlatform(platformProp);
  const held = usePressedKeys(highlightPressed);
  const [query, setQuery] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);

  const heldMods = useMemo(() => [...held].filter(key => ["meta", "ctrl", "alt", "shift"].includes(key)), [held]);
  const words = query.trim().toLowerCase().replace(/[⌘]/g, " cmd ").replace(/[⇧]/g, " shift ").replace(/[⌥]/g, " alt ").replace(/[⌃]/g, " ctrl ").split(/[\s+]+/).filter(Boolean);
  const aliases: Record<string, string> = { cmd: "command", command: "command", ctrl: "control", control: "control", opt: "option", option: platform === "mac" ? "option" : "alt", alt: platform === "mac" ? "option" : "alt", win: "windows", esc: "escape", return: platform === "mac" ? "return" : "enter" };

  const filtered = groups.map(group => ({
    ...group,
    items: group.items.filter(item => {
      if (!words.length) return true;
      const tokens = shortcutTokens(item.shortcut, platform);
      const haystack = `${item.label} ${group.label} ${item.keywords ?? ""} ${tokens.map(token => `${token.spoken} ${token.label}`).join(" ")}`.toLowerCase();
      return words.every(word => haystack.includes(aliases[word] ?? word) || haystack.includes(word));
    }),
  })).filter(group => group.items.length);

  const total = filtered.reduce((sum, group) => sum + group.items.length, 0);
  const rowTransition = reduced ? { duration: 0 } : motionTokens.spring.smooth;

  return <section className={[styles.list, className].filter(Boolean).join(" ")} aria-label={label}>
    {searchable && <div className={styles.searchRow}>
      <Search className={styles.searchIcon} size={16} strokeWidth={1.75} aria-hidden="true" />
      <input ref={searchRef} className={styles.search} type="search" placeholder={searchPlaceholder} value={query} aria-label={searchPlaceholder} onChange={event => setQuery(event.target.value)}
        onKeyDown={event => { if (event.key === "Escape" && query) { event.preventDefault(); setQuery(""); } }} autoComplete="off" spellCheck={false} />
      <span className={styles.count} aria-live="polite">{query ? `${total} 个结果` : ""}</span>
      <AnimatePresence initial={false}>
        {query && <motion.button key="clear" type="button" className={styles.clear} aria-label="清空搜索" onClick={() => { setQuery(""); searchRef.current?.focus(); }}
          initial={{ opacity: 0, scale: .6 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: .6, transition: { duration: .1 } }} transition={reduced ? { duration: 0 } : motionTokens.spring.snappy}>
          <X size={14} strokeWidth={1.75} aria-hidden="true" />
        </motion.button>}
      </AnimatePresence>
    </div>}

    <div className={styles.groups}>
      <AnimatePresence initial={false}>
        {filtered.map(group => <motion.section key={group.label} className={styles.group} aria-label={group.label}
          initial={reduced ? { opacity: 0 } : { opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={reduced ? { opacity: 0 } : { opacity: 0, height: 0 }} transition={rowTransition}>
          <h4 className={styles.groupLabel}>{group.label}</h4>
          <ul className={styles.rows}>
            <AnimatePresence initial={false}>
              {group.items.map(item => {
                const tokens = shortcutTokens(item.shortcut, platform);
                const ids = tokens.map(token => token.id);
                const match = held.size > 0 && ids.every(key => held.has(key));
                const dim = heldMods.length > 0 && !match && !heldMods.every(mod => ids.includes(mod));
                return <motion.li key={item.label} className={styles.item} data-match={match || undefined} data-dim={dim || undefined}
                  initial={reduced ? { opacity: 0 } : { opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={reduced ? { opacity: 0 } : { opacity: 0, height: 0 }} transition={rowTransition}>
                  <span className={styles.itemInner}>
                    <span className={styles.itemLabel}>{item.label}</span>
                    <ShortcutKeys shortcut={item.shortcut} platform={platform} pressed={held} size="sm" />
                  </span>
                </motion.li>;
              })}
            </AnimatePresence>
          </ul>
        </motion.section>)}
      </AnimatePresence>
      {total === 0 && <p className={styles.empty}>没有匹配“{query.trim()}”的快捷键</p>}
    </div>
  </section>;
}

export default ShortcutRecorder;
