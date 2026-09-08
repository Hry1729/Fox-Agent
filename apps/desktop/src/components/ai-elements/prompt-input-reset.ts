/** Clear submitted text without resetting model/assistant selectors in the form. */
export function clearSubmittedPromptText(form: Pick<HTMLFormElement, 'querySelector'>) {
  const message = form.querySelector<HTMLInputElement | HTMLTextAreaElement>(
    'textarea[name="message"], input[name="message"]',
  )
  if (message) message.value = ''
}
