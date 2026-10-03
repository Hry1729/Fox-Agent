import { describe, expect, test } from 'bun:test'
import {
  DEFAULT_PROMPT_SEND_SHORTCUT,
  PROMPT_SEND_SHORTCUT_KEY,
  insertPromptNewline,
  normalizePromptSendShortcut,
  promptEnterIsComposing,
  readPromptSendShortcut,
  resolvePromptEnterAction,
} from '../src/components/ai-elements/prompt-send-shortcut'

describe('send shortcut preference', () => {
  test('defaults to Enter sending when nothing is stored', () => {
    expect(readPromptSendShortcut({ getItem: () => null })).toBe('enter')
    expect(DEFAULT_PROMPT_SEND_SHORTCUT).toBe('enter')
  })

  test('keeps the legacy Ctrl/Cmd + Enter value working', () => {
    expect(readPromptSendShortcut({ getItem: () => 'mod-enter' })).toBe('mod-enter')
  })

  test('normalizes unknown, empty and malformed values to Enter sending', () => {
    for (const stored of ['', 'sEND', 'mod+enter', 'ctrl-enter', 'true']) {
      expect(normalizePromptSendShortcut(stored)).toBe('enter')
    }
    expect(normalizePromptSendShortcut(undefined)).toBe('enter')
    expect(normalizePromptSendShortcut(null)).toBe('enter')
  })

  test('a storage that refuses to be read falls back instead of throwing', () => {
    expect(readPromptSendShortcut({ getItem: () => { throw new Error('blocked') } })).toBe('enter')
    expect(readPromptSendShortcut(null)).toBe('enter')
  })

  test('the preference key is the one the settings page writes', () => {
    expect(PROMPT_SEND_SHORTCUT_KEY).toBe('fox.preferences.sendKey')
  })
})

describe('enter resolution', () => {
  test('Enter sends and Ctrl/Cmd + Enter breaks the line by default', () => {
    expect(resolvePromptEnterAction({ key: 'Enter' }, 'enter')).toBe('send')
    expect(resolvePromptEnterAction({ key: 'Enter', ctrlKey: true }, 'enter')).toBe('newline')
    expect(resolvePromptEnterAction({ key: 'Enter', metaKey: true }, 'enter')).toBe('newline')
  })

  test('the legacy preference swaps the two without losing either', () => {
    expect(resolvePromptEnterAction({ key: 'Enter' }, 'mod-enter')).toBe('newline')
    expect(resolvePromptEnterAction({ key: 'Enter', ctrlKey: true }, 'mod-enter')).toBe('send')
    expect(resolvePromptEnterAction({ key: 'Enter', metaKey: true }, 'mod-enter')).toBe('send')
  })

  test('Shift + Enter always breaks the line, in both modes', () => {
    for (const shortcut of ['enter', 'mod-enter'] as const) {
      expect(resolvePromptEnterAction({ key: 'Enter', shiftKey: true }, shortcut)).toBe('newline')
      expect(resolvePromptEnterAction({ key: 'Enter', shiftKey: true, ctrlKey: true }, shortcut)).toBe('newline')
    }
  })

  test('an IME confirmation never sends', () => {
    expect(promptEnterIsComposing({ key: 'Enter', isComposing: true })).toBe(true)
    expect(promptEnterIsComposing({ key: 'Enter', keyCode: 229 })).toBe(true)
    expect(promptEnterIsComposing({ key: 'Enter', keyCode: 13 })).toBe(false)
    expect(resolvePromptEnterAction({ key: 'Enter', isComposing: true }, 'enter')).toBe('default')
    expect(resolvePromptEnterAction({ key: 'Enter', keyCode: 229 }, 'enter')).toBe('default')
  })

  test('non-Enter keys and Alt + Enter stay with the browser', () => {
    expect(resolvePromptEnterAction({ key: 'a' }, 'enter')).toBe('default')
    expect(resolvePromptEnterAction({ key: 'Escape' }, 'enter')).toBe('default')
    expect(resolvePromptEnterAction({ key: 'Enter', altKey: true }, 'enter')).toBe('default')
  })
})

describe('newline insertion', () => {
  test('inserts at the caret and leaves the caret after the newline', () => {
    expect(insertPromptNewline('hello', 2, 2)).toEqual({ value: 'he\nllo', caret: 3 })
  })

  test('appends at the end of an empty or fully consumed draft', () => {
    expect(insertPromptNewline('', 0, 0)).toEqual({ value: '\n', caret: 1 })
    expect(insertPromptNewline('ab', 2, 2)).toEqual({ value: 'ab\n', caret: 3 })
  })

  test('replaces the current selection instead of appending', () => {
    expect(insertPromptNewline('hello', 1, 3)).toEqual({ value: 'h\nlo', caret: 2 })
    expect(insertPromptNewline('hello', 5, 0)).toEqual({ value: '\n', caret: 1 })
  })

  test('clamps out-of-range offsets instead of corrupting the draft', () => {
    expect(insertPromptNewline('ab', -5, 99)).toEqual({ value: '\n', caret: 1 })
    expect(insertPromptNewline('ab', Number.NaN, Number.NaN)).toEqual({ value: 'ab\n', caret: 3 })
  })
})
