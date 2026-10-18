import { describe, expect, test } from 'bun:test'
import { navigationTarget, recordNavigation, type NavigationHistory } from '../src/features/workspace/navigation-history'

const entry = (key: string, draft = '') => ({ key, route: { key, draft } })
const empty = (): NavigationHistory<{ key: string; draft: string }> => ({ entries: [], index: -1 })

describe('workspace history', () => {
  test('back and forward preserve distinct conversations and document routes', () => {
    let history = empty()
    for (const key of ['chat:a', 'chat:b', 'library:doc-1']) history = recordNavigation(history, entry(key))
    expect(navigationTarget(history, -1)?.route.key).toBe('chat:b')
    history = { ...history, index: 1 }
    expect(navigationTarget(history, 1)?.route.key).toBe('library:doc-1')
    expect(navigationTarget(history, -1)?.route.key).toBe('chat:a')
  })
  test('typing updates the current snapshot without adding a page', () => {
    let history = recordNavigation(empty(), entry('chat:draft', 'first'))
    history = recordNavigation(history, entry('chat:draft', 'unsent text'))
    expect(history.entries).toHaveLength(1)
    expect(history.entries[0].route.draft).toBe('unsent text')
    expect(navigationTarget(history, -1)).toBeUndefined()
    expect(navigationTarget(history, 1)).toBeUndefined()
  })
  test('a fresh destination after going back discards the forward branch', () => {
    let history = recordNavigation(recordNavigation(empty(), entry('chat')), entry('agents'))
    history = { ...history, index: 0 }
    history = recordNavigation(history, entry('settings'))
    expect(history.entries.map(item => item.key)).toEqual(['chat', 'settings'])
    expect(navigationTarget(history, 1)).toBeUndefined()
  })
  test('history is bounded', () => {
    let history = empty()
    for (let index = 0; index < 150; index++) history = recordNavigation(history, entry(String(index)))
    expect(history.entries).toHaveLength(100)
    expect(history.index).toBe(99)
    expect(history.entries[0].key).toBe('50')
  })
})
