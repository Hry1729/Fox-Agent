export const FOX_ASSISTANT_AVATAR = '/avatars/fox-assistant.png'

export function FoxAssistantAvatar({ size = 'sm' }: { size?: 'sm' | 'md' | 'lg' }) {
  return <img className={`fox-assistant-avatar is-${size}`} src={FOX_ASSISTANT_AVATAR} alt="" aria-hidden="true" width={28} height={28} />
}

/** Keep a readable base below the blue light, including without text clipping. */
export function RunStatusText({ text, active = true }: { text: string; active?: boolean }) {
  return <span className="fox-run-status-text" data-active={active}>
    <span className="fox-run-status-label">{text}</span>
    {active && <span className="fox-run-status-light" aria-hidden="true">{text}</span>}
  </span>
}
