import type { AppNotificationRecord, NotificationPreferencesRecord } from '@/features/conversations/model/types'

export type NotificationSoundId = 'soft' | 'chime' | 'pop' | 'signal'

export const NOTIFICATION_SOUND_OPTIONS: Array<{ id: NotificationSoundId; label: string; description: string }> = [
  { id: 'soft', label: '柔和', description: '轻柔的单音提示' },
  { id: 'chime', label: '清脆', description: '两段上扬铃音' },
  { id: 'pop', label: '轻点', description: '短促明快的提示音' },
  { id: 'signal', label: '信号', description: '醒目的双段信号音' },
]

export const DEFAULT_NOTIFICATION_SOUND: NotificationSoundId = 'soft'
export const FOX_IN_APP_NOTIFICATION_EVENT = 'fox:in-app-notification'
export const FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT = 'fox:notification-preferences-changed'

export interface InAppNotificationEventDetail {
  notification: Pick<AppNotificationRecord, 'id' | 'kind' | 'title' | 'body'>
  soundId?: NotificationSoundId
  forceSound?: boolean
}

export function normalizeNotificationSoundId(value?: string | null): NotificationSoundId {
  return NOTIFICATION_SOUND_OPTIONS.some((option) => option.id === value)
    ? value as NotificationSoundId
    : DEFAULT_NOTIFICATION_SOUND
}

export function playNotificationSound(soundId: NotificationSoundId = DEFAULT_NOTIFICATION_SOUND) {
  if (typeof window === 'undefined') return
  try {
    const AudioContextConstructor = window.AudioContext
      ?? (window as Window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
    if (!AudioContextConstructor) return
    const context = new AudioContextConstructor()
    const startAt = context.currentTime + 0.01
    const patterns: Record<NotificationSoundId, Array<{ frequency: number; delay: number; duration: number; gain: number; type: OscillatorType }>> = {
      soft: [{ frequency: 660, delay: 0, duration: 0.18, gain: 0.052, type: 'sine' }],
      chime: [
        { frequency: 523.25, delay: 0, duration: 0.16, gain: 0.045, type: 'sine' },
        { frequency: 783.99, delay: 0.12, duration: 0.24, gain: 0.04, type: 'sine' },
      ],
      pop: [
        { frequency: 420, delay: 0, duration: 0.08, gain: 0.04, type: 'triangle' },
        { frequency: 720, delay: 0.055, duration: 0.1, gain: 0.035, type: 'triangle' },
      ],
      signal: [
        { frequency: 880, delay: 0, duration: 0.09, gain: 0.032, type: 'square' },
        { frequency: 660, delay: 0.13, duration: 0.11, gain: 0.028, type: 'square' },
      ],
    }
    let stopAfter = 0
    for (const tone of patterns[soundId]) {
      const oscillator = context.createOscillator()
      const gain = context.createGain()
      const toneStart = startAt + tone.delay
      const toneEnd = toneStart + tone.duration
      stopAfter = Math.max(stopAfter, tone.delay + tone.duration)
      oscillator.type = tone.type
      oscillator.frequency.setValueAtTime(tone.frequency, toneStart)
      gain.gain.setValueAtTime(0.0001, toneStart)
      gain.gain.exponentialRampToValueAtTime(tone.gain, toneStart + Math.min(0.018, tone.duration / 3))
      gain.gain.exponentialRampToValueAtTime(0.0001, toneEnd)
      oscillator.connect(gain)
      gain.connect(context.destination)
      oscillator.start(toneStart)
      oscillator.stop(toneEnd)
    }
    window.setTimeout(() => void context.close(), Math.ceil((stopAfter + 0.08) * 1000))
  } catch {
    // WebViews may require a recent user gesture. Notification delivery remains available.
  }
}

export function dispatchNotificationPreview(soundId: NotificationSoundId) {
  if (typeof window === 'undefined') return
  const detail: InAppNotificationEventDetail = {
    notification: {
      id: `notification-test-${Date.now()}`,
      kind: 'completed',
      title: 'Fox 测试通知',
      body: '通知卡片、倒计时进度条和提示音均工作正常。',
    },
    soundId,
    forceSound: true,
  }
  window.dispatchEvent(new CustomEvent<InAppNotificationEventDetail>(FOX_IN_APP_NOTIFICATION_EVENT, { detail }))
}

export function dispatchNotificationPreferencesChanged(preferences: NotificationPreferencesRecord) {
  if (typeof window === 'undefined') return
  window.dispatchEvent(new CustomEvent<NotificationPreferencesRecord>(FOX_NOTIFICATION_PREFERENCES_CHANGED_EVENT, { detail: preferences }))
}
