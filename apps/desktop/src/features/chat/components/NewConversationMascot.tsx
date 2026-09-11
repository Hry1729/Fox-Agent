import { useEffect, useState } from 'react'
import { idleMascots, mascotAt } from '../mascot-library'

/** The rotating illustration belongs only to the empty, new-conversation page. */
export function NewConversationMascot() {
  const [index, setIndex] = useState(0)
  const [visible, setVisible] = useState(true)

  useEffect(() => {
    let interval: number | undefined
    const syncVisibility = () => {
      window.clearInterval(interval)
      setVisible(!document.hidden)
      if (document.hidden) return
      interval = window.setInterval(() => setIndex((current) => {
        const offset = 1 + Math.floor(Math.random() * Math.max(1, idleMascots.length - 1))
        return (current + offset) % idleMascots.length
      }), 10000)
    }
    syncVisibility()
    document.addEventListener('visibilitychange', syncVisibility)
    return () => {
      window.clearInterval(interval)
      document.removeEventListener('visibilitychange', syncVisibility)
    }
  }, [])

  const src = mascotAt(idleMascots, index, '/mascot/fox/idle/fox_sit_nicely.png')
  return <div className="fox-empty-mascot" data-paused={!visible} aria-hidden="true">
    <img key={src} src={src} alt="" />
  </div>
}
