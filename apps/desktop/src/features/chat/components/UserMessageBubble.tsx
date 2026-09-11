import { useId, useLayoutEffect, useRef, useState } from 'react'
import { ChevronDown, ChevronUp } from 'lucide-react'
import { MessageContent } from '@/components/ai-elements/message'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import type { UserProfile } from '@/features/profile/user-profile'

export function UserMessageAvatar({ profile }: { profile: UserProfile }) {
  return <Avatar className="fox-user-avatar" aria-label={profile.name}>
    <AvatarImage src={profile.avatar} alt="" />
    <AvatarFallback>{profile.initial}</AvatarFallback>
  </Avatar>
}

export function UserMessageBubble({ children }: { children: string }) {
  const textRef = useRef<HTMLDivElement>(null)
  const textId = useId()
  const [expanded, setExpanded] = useState(false)
  const [overflows, setOverflows] = useState(false)
  useLayoutEffect(() => {
    const text = textRef.current
    if (!text) return
    const measure = () => setOverflows(text.scrollHeight > 218)
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(text)
    return () => observer.disconnect()
  }, [children])
  return <MessageContent className={`fox-user-bubble ${expanded ? 'is-expanded' : ''}`}>
    <div id={textId} ref={textRef} className="fox-user-bubble-text">{children}</div>
    {overflows && <button type="button" className="fox-user-bubble-toggle" aria-expanded={expanded} aria-controls={textId} onClick={() => setExpanded((value) => !value)}>
      <span>{expanded ? '收起' : '展开全部文字'}</span>{expanded ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
    </button>}
  </MessageContent>
}
