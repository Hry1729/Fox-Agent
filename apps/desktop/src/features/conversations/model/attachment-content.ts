const MAX_EXTRACTED_ATTACHMENT_CHARACTERS = 120_000
const MAX_PDF_PAGES = 200

export interface SubmittedAttachmentFile {
  filename?: string
  mediaType?: string
  url?: string
}

export async function extractedAttachmentContext(files: SubmittedAttachmentFile[]) {
  const sections: string[] = []
  for (const file of files) {
    if (!isPdf(file) || !file.url?.startsWith('data:')) continue
    let text = ''
    try {
      text = await extractPdfText(file.url)
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause)
      text = `[Fox could not extract this PDF's text: ${message}]`
    }
    if (!text) text = '[This PDF contains no extractable text. It may be a scanned document and require OCR.]'
    sections.push([
      `PDF attachment: ${file.filename ?? 'document.pdf'}`,
      text,
    ].join('\n'))
  }
  if (!sections.length) return ''
  return `\n\nFox extracted attachment content (the original files are also retained):\n\n${sections.join('\n\n---\n\n')}`
}

function isPdf(file: SubmittedAttachmentFile) {
  return file.mediaType?.split(';')[0].trim().toLowerCase() === 'application/pdf'
    || file.filename?.toLowerCase().endsWith('.pdf') === true
}

async function extractPdfText(dataUrl: string) {
  const [pdfjs, workerModule] = await Promise.all([
    import('pdfjs-dist'),
    import('pdfjs-dist/build/pdf.worker.min.mjs?url'),
  ])
  pdfjs.GlobalWorkerOptions.workerSrc = workerModule.default
  const loadingTask = pdfjs.getDocument({
    data: dataUrlBytes(dataUrl),
    stopAtErrors: false,
    enableXfa: false,
    isOffscreenCanvasSupported: false,
    isImageDecoderSupported: false,
  })
  try {
    const document = await loadingTask.promise
    const pages: string[] = []
    let characters = 0
    const pageCount = Math.min(document.numPages, MAX_PDF_PAGES)
    for (let pageNumber = 1; pageNumber <= pageCount; pageNumber += 1) {
      const page = await document.getPage(pageNumber)
      const content = await page.getTextContent()
      const text = content.items
        .map((item) => ('str' in item ? item.str : ''))
        .filter(Boolean)
        .join(' ')
        .replace(/\s+/g, ' ')
        .trim()
      page.cleanup()
      if (!text) continue
      const remaining = MAX_EXTRACTED_ATTACHMENT_CHARACTERS - characters
      if (remaining <= 0) break
      const pageText = text.slice(0, remaining)
      pages.push(`[Page ${pageNumber}]\n${pageText}`)
      characters += pageText.length
      if (pageText.length < text.length) break
    }
    return pages.join('\n\n')
  } finally {
    await loadingTask.destroy()
  }
}

function dataUrlBytes(dataUrl: string) {
  const separator = dataUrl.indexOf(',')
  if (separator < 0) throw new Error('PDF attachment data is invalid')
  const header = dataUrl.slice(0, separator)
  const payload = dataUrl.slice(separator + 1)
  if (!header.includes(';base64')) {
    return new TextEncoder().encode(decodeURIComponent(payload))
  }
  const binary = atob(payload)
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index)
  return bytes
}
