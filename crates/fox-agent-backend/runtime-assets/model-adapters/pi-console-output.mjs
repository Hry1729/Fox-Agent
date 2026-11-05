import { execFileSync } from 'node:child_process';

let consoleEncoding;
function windowsConsoleEncoding() {
  if (consoleEncoding) return consoleEncoding;
  try {
    const output = execFileSync('cmd.exe', ['/d', '/c', 'chcp'], { windowsHide: true, encoding: 'utf8' });
    const codepage = output.match(/\d+/)?.[0];
    const label = { 936: 'gbk', 932: 'shift_jis', 949: 'euc-kr', 950: 'big5', 65001: 'utf-8' }[codepage] ?? `windows-${codepage}`;
    new TextDecoder(label);
    consoleEncoding = label;
  } catch {
    consoleEncoding = 'utf-8';
  }
  return consoleEncoding;
}

export class FoxConsoleDecoder {
  pending = Buffer.alloc(0);
  fallback;
  constructor(encoding = windowsConsoleEncoding()) {
    this.encoding = encoding;
  }
  decode(data = Buffer.alloc(0), final = false) {
    if (this.fallback) return this.fallback.decode(data, { stream: !final });
    this.pending = Buffer.concat([this.pending, data]);
    try {
      const text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true })
        .decode(this.pending, { stream: !final });
      this.pending = this.pending.subarray(Buffer.byteLength(text));
      return text;
    } catch {
      this.fallback = new TextDecoder(this.encoding);
      const text = this.fallback.decode(this.pending, { stream: !final });
      this.pending = Buffer.alloc(0);
      return text;
    }
  }
}

// Normalize each pipe before Pi accumulates or persists it. stdout and stderr
// can use different encodings, and a multibyte character may span data events.
export function pipeFoxConsoleOutput(stream, onData) {
  if (process.platform !== 'win32') {
    stream?.on('data', onData);
    return () => {};
  }
  const decoder = new FoxConsoleDecoder();
  const emit = (text) => { if (text) onData(Buffer.from(text, 'utf8')); };
  stream?.on('data', (data) => emit(decoder.decode(data)));
  return () => emit(decoder.decode(undefined, true));
}
