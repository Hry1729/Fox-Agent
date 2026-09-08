import { expect, test } from 'bun:test'
import { clearSubmittedPromptText } from '../src/components/ai-elements/prompt-input-reset'

test('clearing submitted text never resets the assistant or model selection', () => {
  const message = { value: 'message to submit' }
  const assistant = { value: 'fox-general' }
  const model = { value: 'configured-model' }
  const selectors: string[] = []
  const form = {
    querySelector: (selector: string) => { selectors.push(selector); return message },
    reset: () => { throw new Error('reset would change the current conversation') },
  }
  clearSubmittedPromptText(form as unknown as HTMLFormElement)
  expect(message.value).toBe('')
  expect(assistant.value).toBe('fox-general')
  expect(model.value).toBe('configured-model')
  expect(selectors).toEqual(['textarea[name="message"], input[name="message"]'])
  message.value = 'new input after submission'
  expect(message.value).toBe('new input after submission')
})

test('a form without a message control is left untouched', () => {
  expect(() => clearSubmittedPromptText({ querySelector: () => null })).not.toThrow()
})
