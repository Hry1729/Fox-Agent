import { useEffect, useMemo, useState } from 'react'

export interface ExpertIconOption {
  id: string
  name: string
  src: string
}

const fallbackIcons: ExpertIconOption[] = [
  { id: 'fox-default', name: 'Fox 默认', src: '/mascot/fox_magic.png' },
]

function validIconOption(value: unknown): value is ExpertIconOption {
  if (!value || typeof value !== 'object') return false
  const option = value as Partial<ExpertIconOption>
  return typeof option.id === 'string'
    && typeof option.name === 'string'
    && typeof option.src === 'string'
    && option.src.startsWith('/')
}

export function useExpertIcons(enabled: boolean) {
  const [items, setItems] = useState<ExpertIconOption[]>(fallbackIcons)
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (!enabled) return
    let active = true
    setLoading(true)
    void fetch('/expert-icons/index.json', { cache: 'no-store' })
      .then((response) => {
        if (!response.ok) throw new Error(`expert icon manifest returned ${response.status}`)
        return response.json()
      })
      .then((value: unknown) => {
        if (!active || !Array.isArray(value)) return
        const next = value.filter(validIconOption)
        setItems(next.length ? next : fallbackIcons)
      })
      .catch(() => {
        if (active) setItems(fallbackIcons)
      })
      .finally(() => {
        if (active) setLoading(false)
      })
    return () => { active = false }
  }, [enabled])

  return useMemo(() => ({ items, loading }), [items, loading])
}
