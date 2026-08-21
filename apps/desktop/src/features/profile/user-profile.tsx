import { useEffect, useState } from 'react'
import { Check, Pencil } from 'lucide-react'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { profileAvatarUrls } from '@/generated/profile-avatars'

const profileStorageKey = 'fox.profile.local'
const profileChangedEvent = 'fox:profile-changed'

export const defaultProfileAvatars = profileAvatarUrls
const defaultProfileAvatar = defaultProfileAvatars[0] ?? '/mascot/fox_magic.png'

interface StoredUserProfile { name?: string; avatar?: string }
export interface UserProfile { name: string; avatar: string; initial: string }

function readStoredProfile(): StoredUserProfile {
  if (typeof window === 'undefined') return {}
  try {
    const parsed = JSON.parse(window.localStorage.getItem(profileStorageKey) ?? '{}') as StoredUserProfile
    return {
      name: typeof parsed.name === 'string' ? parsed.name.trim() : undefined,
      avatar: typeof parsed.avatar === 'string' ? parsed.avatar : undefined,
    }
  } catch {
    return {}
  }
}

function resolveAvailableAvatar(avatar?: string | null) {
  const value = avatar?.trim()
  if (!value) return undefined
  if (value.startsWith('/avatars/defaults/') && !defaultProfileAvatars.includes(value)) return undefined
  return value
}

export function useUserProfile(fallback?: { name?: string | null; avatar?: string | null }) {
  const [stored, setStored] = useState<StoredUserProfile>(() => readStoredProfile())
  useEffect(() => {
    let disposed = false
    const refresh = async () => {
      if (!desktopRuntimeAvailable) {
        if (!disposed) setStored(readStoredProfile())
        return
      }
      try {
        let profile = await desktopClient.getUserProfile()
        if (!profile) {
          const legacy = readStoredProfile()
          if (legacy.name && legacy.avatar) {
            profile = await desktopClient.saveUserProfile({ name: legacy.name, avatar: legacy.avatar })
            window.localStorage.removeItem(profileStorageKey)
          }
        }
        if (!disposed) setStored(profile ?? {})
      } catch {
        if (!disposed) setStored(readStoredProfile())
      }
    }
    void refresh()
    window.addEventListener('storage', refresh)
    window.addEventListener(profileChangedEvent, refresh)
    return () => {
      disposed = true
      window.removeEventListener('storage', refresh)
      window.removeEventListener(profileChangedEvent, refresh)
    }
  }, [])
  const name = stored.name || fallback?.name?.trim() || 'Fox 用户'
  const avatar = resolveAvailableAvatar(stored.avatar) || resolveAvailableAvatar(fallback?.avatar) || defaultProfileAvatar
  const profile: UserProfile = { name, avatar, initial: name.charAt(0).toUpperCase() || 'F' }
  const save = async (next: Pick<UserProfile, 'name' | 'avatar'>) => {
    const value = { name: next.name.trim(), avatar: next.avatar }
    const saved = desktopRuntimeAvailable
      ? await desktopClient.saveUserProfile(value)
      : value
    if (desktopRuntimeAvailable) window.localStorage.removeItem(profileStorageKey)
    else window.localStorage.setItem(profileStorageKey, JSON.stringify(saved))
    setStored(saved)
    window.dispatchEvent(new Event(profileChangedEvent))
  }
  return { profile, save }
}

export function UserProfileDialog({ open, onOpenChange, profile, detail, onSave }: {
  open: boolean
  onOpenChange: (open: boolean) => void
  profile: UserProfile
  detail?: string
  onSave: (profile: Pick<UserProfile, 'name' | 'avatar'>) => void | Promise<void>
}) {
  const [name, setName] = useState(profile.name)
  const [avatar, setAvatar] = useState(profile.avatar)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (!open) return
    setName(profile.name)
    setAvatar(profile.avatar)
    setError(null)
  }, [open, profile.avatar, profile.name])
  const submit = async () => {
    if (!name.trim() || saving) return
    setSaving(true)
    setError(null)
    try {
      await onSave({ name: name.trim(), avatar })
      onOpenChange(false)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : '个人资料保存失败')
    } finally {
      setSaving(false)
    }
  }
  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent className="fox-user-profile-dialog sm:max-w-md">
      <DialogHeader><DialogTitle>编辑个人资料</DialogTitle><DialogDescription>选择 Fox 内置头像并设置在这台设备上显示的用户名。</DialogDescription></DialogHeader>
      <div className="fox-user-profile-preview"><Avatar><AvatarImage src={avatar} alt="" /><AvatarFallback>{profile.initial}</AvatarFallback></Avatar><span><strong>{name.trim() || profile.name}</strong><small>{detail || 'Fox 本地用户'}</small></span></div>
      <label className="fox-user-profile-field"><span>用户名</span><Input value={name} maxLength={32} autoFocus onChange={(event) => setName(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') void submit() }} /><small>{name.trim().length}/32</small></label>
      <div className="fox-user-profile-field"><span>选择头像</span><div className="fox-avatar-picker" role="radiogroup" aria-label="选择头像">{defaultProfileAvatars.map((src) => <button key={src} type="button" role="radio" aria-checked={avatar === src} className={avatar === src ? 'is-active' : ''} onClick={() => setAvatar(src)}><img src={src} alt="" />{avatar === src && <i><Check /></i>}</button>)}</div></div>
      {error && <p className="fox-user-profile-error">{error}</p>}
      <DialogFooter><Button variant="outline" disabled={saving} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={!name.trim() || saving} onClick={() => void submit()}><Pencil />{saving ? '保存中' : '保存资料'}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}
