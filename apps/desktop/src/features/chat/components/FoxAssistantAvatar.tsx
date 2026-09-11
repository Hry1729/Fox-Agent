export const FOX_ASSISTANT_AVATAR = '/avatars/fox-assistant.png'

export function FoxAssistantAvatar({ size = 'sm' }: { size?: 'sm' | 'md' | 'lg' }) {
  return <img className={`fox-assistant-avatar is-${size}`} src={FOX_ASSISTANT_AVATAR} alt="" aria-hidden="true" width={28} height={28} />
}

/** Visible without text-clipping support and animated entirely by the browser. */
export function RunStatusText({ text, active = true }: { text: string; active?: boolean }) {
  return <span className="fox-run-status-text" data-active={active}>
    <span>{text}</span>
    {active && <span className="fox-run-status-dots" aria-hidden="true"><i /><i /><i /></span>}
  </span>
}
