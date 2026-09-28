export const FOX_ASSISTANT_AVATAR = '/avatars/fox-assistant.png'

export function FoxAssistantAvatar({ size = 'sm' }: { size?: 'sm' | 'md' | 'lg' }) {
  return <img className={`fox-assistant-avatar is-${size}`} src={FOX_ASSISTANT_AVATAR} alt="" aria-hidden="true" width={28} height={28} />
}

/** The label paints both its readable base and moving highlight, without duplicate glyphs. */
export function RunStatusText({ text, active = true }: { text: string; active?: boolean }) {
  return <span className="fox-run-status-text" data-active={active}>
    <span className="fox-run-status-label">{text}</span>
  </span>
}
