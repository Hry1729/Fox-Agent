import { useCallback, useEffect, useState } from 'react'
import { CornerDownLeft, MoreHorizontal, Quote, Reply, SmilePlus, Star, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { copyMessageResponse } from '@/components/assistant-ui/elements/message-actions'
import { FoxCopyMessageButton, FoxMarkUnreadButton } from '@/features/chat/thread/fox-message-actions'
import { StreamingText } from '@/features/chat/components/StreamingText'
import type { ContactBubbleMarkRecord, ContactMessageQuoteRecord } from '@/features/conversations/model/types'
import { contacts, REACTIONS, type Companion, type FavoriteEntry } from './api'
import { ContactAvatar } from './chat-controls'
import { formatSearchTime } from './chat-search'
import { showContactError, useContacts } from './context'
import './contact-chat.css'

/** C07: one bubble's marks. The Host answer is the truth, so a toggle updates
 * from the returned record and publishes it for the other views of this chat
 * (the favorites page); a later detail load re-syncs to the stored one. */
function useBubbleMark(bubbleId: string, initial: ContactBubbleMarkRecord | undefined) {
  const ctx = useContacts()
  const published = ctx?.bubbleMarkUpdates?.get(bubbleId)
  const source = published ?? initial
  const [mark, setMark] = useState(source)
  const [stored, setStored] = useState(source)
  const [busy, setBusy] = useState(false)
  if (stored !== source) { setStored(source); setMark(source) }
  const publish = ctx?.publishBubbleMark
  const run = useCallback(async (action: () => Promise<ContactBubbleMarkRecord>) => {
    setBusy(true)
    try {
      const next = await action()
      setMark(next)
      publish?.(next)
    } catch (error) { showContactError(error) } finally { setBusy(false) }
  }, [publish])
  return { mark, busy, run }
}

/** The six reactions（SPEC item 11：Radix DropdownMenu 取代原生 details——外部指针、
 * Escape、选中都会关闭；open 状态由气泡持有，两个菜单天然互斥）。
 * Clicking the stored one clears it, another replaces it. */
function ReactionPicker({ reaction, busy, onReact, open, onOpenChange }: {
  reaction: string | null | undefined
  busy: boolean
  onReact: (reaction: string) => void
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  return <DropdownMenu open={open} onOpenChange={onOpenChange}>
    <DropdownMenuTrigger asChild>
      <button type="button" className="fox-thread-action-button" aria-label="表情反馈" title="表情反馈"><SmilePlus size={13} /></button>
    </DropdownMenuTrigger>
    <DropdownMenuContent side="top" align="end" aria-label="选择一个表情"
      className="fox-contact-reaction-menu w-auto min-w-0 max-h-none overflow-visible">
      {REACTIONS.map(item => <DropdownMenuItem key={item}
        className={reaction === item ? 'fox-contact-reaction-item is-chosen' : 'fox-contact-reaction-item'}
        disabled={busy}
        aria-label={reaction === item ? `取消 ${item}` : `回应 ${item}`} title={reaction === item ? '取消这个表情' : item}
        onSelect={() => onReact(item)}>{item}</DropdownMenuItem>)}
    </DropdownMenuContent>
  </DropdownMenu>
}

/** 引用回复与收藏。只有已落库的气泡才有这个菜单。 */
function BubbleMenu({ favorited, busy, onQuote, onFavorite, open, onOpenChange }: {
  favorited: boolean
  busy: boolean
  onQuote: () => void
  onFavorite: () => void
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  return <DropdownMenu open={open} onOpenChange={onOpenChange}>
    <DropdownMenuTrigger asChild>
      <button type="button" className="fox-thread-action-button" aria-label="更多消息操作" title="更多"><MoreHorizontal size={13} /></button>
    </DropdownMenuTrigger>
    <DropdownMenuContent side="top" align="end" className="fox-contact-bubble-menu w-auto">
      <DropdownMenuItem className="fox-contact-bubble-menu-item" onSelect={() => onQuote()}><Reply size={13} />引用回复</DropdownMenuItem>
      <DropdownMenuItem className="fox-contact-bubble-menu-item" disabled={busy} data-favorited={favorited ? 'true' : undefined}
        onSelect={() => onFavorite()}><Star size={13} fill={favorited ? 'currentColor' : 'none'} />{favorited ? '取消收藏' : '收藏'}</DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
}

/** The chosen emoji, beside the bubble. No counts: it is only the user's own. */
function ReactionChip({ reaction, busy, onClear }: { reaction: string; busy: boolean; onClear: () => void }) {
  return <button type="button" className="fox-contact-reaction-chip" disabled={busy}
    aria-label={`已回应 ${reaction}，点击取消`} title="点击取消这个表情" onClick={onClear}>{reaction}</button>
}

export interface ContactReply {
  id: string
  text: string
  createdAt: number
  /** Present only for a committed bubble; a live draft has none. */
  deliveryOrder?: number
}

/** One contact reply: bubble, the user's marks and the hover action row. A live
 * draft or replaceable preview offers copy only — nothing permanent attaches to
 * an identity that is not in `contact_bubbles` yet. */
export function ContactReplyBubble({ contact, reply, initialMark }: {
  contact?: Companion
  reply: ContactReply
  initialMark?: ContactBubbleMarkRecord
}) {
  const ctx = useContacts()
  const durable = reply.deliveryOrder != null && Boolean(contact)
  const { mark, busy, run } = useBubbleMark(reply.id, durable ? initialMark : undefined)
  // 一个气泡同时只开一层菜单（SPEC item 11 的互斥）；菜单开在 portal 里，
  // 焦点离开了气泡 DOM，用 data-menu-open 让操作行保持可见。
  const [openMenu, setOpenMenu] = useState<'reaction' | 'more' | null>(null)
  const favorite = () => contact && run(() => contacts.favorite(contact.id, reply.id, !mark?.favoritedAt))
  const react = (reaction: string) => contact && run(() => contacts.react(contact.id, reply.id, reaction))
  return <section className="fox-contact-message" data-contact-message-id={reply.id}
    data-menu-open={openMenu ? 'true' : undefined}
    data-bubble-id={durable ? reply.id : undefined}
    data-delivery-order={durable ? reply.deliveryOrder : undefined}>
    <div className="fox-contact-bubble"><StreamingText text={reply.text} streaming={false} className="fox-answer-response" /></div>
    {mark?.reaction && <ReactionChip reaction={mark.reaction} busy={busy} onClear={() => react(mark.reaction!)} />}
    <div className="fox-contact-message-meta">
      <FoxCopyMessageButton onCopy={() => copyMessageResponse(reply.text)} />
      {durable && <>
        <ReactionPicker reaction={mark?.reaction} busy={busy} onReact={react}
          open={openMenu === 'reaction'} onOpenChange={next => setOpenMenu(next ? 'reaction' : null)} />
        <BubbleMenu favorited={Boolean(mark?.favoritedAt)} busy={busy}
          onQuote={() => void ctx?.quoteBubble?.(reply.id)} onFavorite={favorite}
          open={openMenu === 'more'} onOpenChange={next => setOpenMenu(next ? 'more' : null)} />
        {contact && <FoxMarkUnreadButton onMark={() => contacts.markUnread(contact.id, reply.id).then(() => ctx?.refresh()).catch(error => { showContactError(error); throw error })} />}
      </>}
    </div>
  </section>
}

/** The same marks on the user's own message of a private chat. */
export function ContactUserMarks({ contact, messageId, getText, initialMark }: {
  contact: Companion
  messageId: string
  getText: () => string
  createdAt: Date
  initialMark?: ContactBubbleMarkRecord
}) {
  const ctx = useContacts()
  const { mark, busy, run } = useBubbleMark(messageId, initialMark)
  const [openMenu, setOpenMenu] = useState<'reaction' | 'more' | null>(null)
  const favorite = () => run(() => contacts.favorite(contact.id, messageId, !mark?.favoritedAt))
  const react = (reaction: string) => run(() => contacts.react(contact.id, messageId, reaction))
  return <div className="fox-user-meta" data-menu-open={openMenu ? 'true' : undefined}>
    {mark?.reaction && <ReactionChip reaction={mark.reaction} busy={busy} onClear={() => react(mark.reaction!)} />}
    <FoxCopyMessageButton onCopy={() => copyMessageResponse(getText())} />
    <ReactionPicker reaction={mark?.reaction} busy={busy} onReact={react}
      open={openMenu === 'reaction'} onOpenChange={next => setOpenMenu(next ? 'reaction' : null)} />
    <BubbleMenu favorited={Boolean(mark?.favoritedAt)} busy={busy}
      onQuote={() => void ctx?.quoteBubble?.(messageId)} onFavorite={favorite}
      open={openMenu === 'more'} onOpenChange={next => setOpenMenu(next ? 'more' : null)} />
  </div>
}

/** The quote area of a sent message. Clicking returns to the original bubble;
 * once the source is gone the summary stays and the jump is refused — it never
 * lands on a different message that happens to sit at the same position. */
export function ContactQuoteRef({ quote, contactName }: { quote: ContactMessageQuoteRecord; contactName: string }) {
  const ctx = useContacts()
  const author = quote.author === 'user' ? '我' : contactName
  if (!quote.available) return <div className="fox-contact-qref is-gone" role="note">
    <Quote size={12} aria-hidden="true" /><b>{author}</b><span>{quote.summary}</span><small>原消息不可用</small>
  </div>
  return <button type="button" className="fox-contact-qref" title="回到原消息"
    onClick={() => ctx?.jumpToMessage?.(quote.bubbleId)}>
    <Quote size={12} aria-hidden="true" /><b>{author}</b><span>{quote.summary}</span>
  </button>
}

/** The composer's quote draft: author plus one line, cancellable. Cancelling
 * removes it from the next send entirely. */
export function ContactQuoteDraft() {
  const ctx = useContacts()
  const draft = ctx?.quoteDraft
  if (!ctx?.privateChat || !draft) return null
  const author = draft.author === 'user' ? '我' : (ctx.current?.displayName ?? '联系人')
  return <div className="fox-contact-quote-draft" role="status">
    <Reply size={13} aria-hidden="true" />
    <b>回复{author}</b>
    <span>{draft.summary}</span>
    <button type="button" className="fox-icon-button" aria-label="取消引用" title="取消引用" onClick={ctx.clearQuote}><X size={13} /></button>
  </div>
}

/** The contact's favorites (C07): the user's own marks, never contact notes. */
export function ContactFavorites({ contact, onBack }: { contact: Companion; onBack?: () => void }) {
  const ctx = useContacts()
  const [entries, setEntries] = useState<FavoriteEntry[] | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const reload = useCallback(async () => {
    try { setEntries(await contacts.favorites(contact.id)); setError('') } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }, [contact.id])
  useEffect(() => { void reload() }, [reload])
  const remove = async (bubbleId: string) => {
    setBusy(true)
    try {
      const next = await contacts.favorite(contact.id, bubbleId, false)
      ctx?.publishBubbleMark?.(next)
      await reload()
    } catch (cause) { showContactError(cause) } finally { setBusy(false) }
  }
  return <div className="fox-contact-favorites" aria-busy={busy}>
    <div className="fox-contact-capabilities-head"><h3>收藏</h3>
      {onBack && <Button size="sm" variant="ghost" onClick={onBack}>返回资料</Button>}</div>
    <p className="fox-contact-capability-hint">只是你自己的标记，不会写进它的笔记，也不会改变它的记忆。</p>
    {error && <p role="alert">{error} <button type="button" onClick={() => void reload()}>重试</button></p>}
    {!entries ? <p>正在读取收藏…</p> : !entries.length ? <p className="fox-contact-capability-hint">还没有收藏过消息。</p>
      : entries.map(entry => <div className="fox-contact-favorite" key={entry.bubbleId}>
        {entry.author === 'user' ? <span className="fox-contact-avatar is-letter" aria-hidden="true">我</span> : <ContactAvatar contact={contact} />}
        <div className="fox-contact-favorite-body">
          <p>{entry.summary}</p>
          <time dateTime={new Date(entry.deliveredAt).toISOString()}>{formatSearchTime(entry.deliveredAt)}</time>
        </div>
        <div className="fox-contact-actions">
          <Button size="sm" variant="ghost" onClick={() => ctx?.jumpToMessage?.(entry.bubbleId)}><CornerDownLeft size={13} />回到消息</Button>
          <Button size="sm" variant="ghost" disabled={busy} aria-label="取消收藏" onClick={() => void remove(entry.bubbleId)}><Star size={13} fill="currentColor" /></Button>
        </div>
      </div>)}
  </div>
}
