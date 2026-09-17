/**
 * Bounded, redacted worker diagnostics.
 *
 * A Kernel worker failure has to be diagnosable from the Host's stderr, but the
 * worker holds provider payloads and request URLs: a raw `error.stack`, or a raw
 * `error.message`/`error.cause`, is arbitrary text that can carry an API key or
 * a slice of model input. These helpers publish only a fixed field list, each
 * capped, with credential-shaped substrings and URL paths removed. Messages
 * remain bounded free text; this is not a guarantee of arbitrary-text secrecy.
 */

/** Cap for one free-text field. */
const MAX_TEXT = 200;
/** Cap for one machine token (error name, errno, cause code). */
const MAX_TOKEN = 64;
/** Cap for the serialized diagnostic line, so flooding stderr stays bounded. */
const MAX_LINE = 900;

const URLS = /\b[a-z][a-z0-9+.-]{0,20}:\/\/\S{0,4096}/gi;
const SECRET_ASSIGNMENT =
  /(?:\b|["'])(?:api[_-]?key|apikey|access[_-]?token|auth[_-]?token|client[_-]?secret|secret|password|passwd|token|key)(["']?)\s*[:=]\s*(?:"[^"]*(?:"|$)|'[^']*(?:'|$)|[^\s"',;}&]+)/gi;
const BEARER = /\bbearer\s+[a-z0-9._~+/-]+=*/gi;
const PROVIDER_KEY = /\bsk-[a-z0-9_-]{6,}\b/gi;

/** One URL becomes `scheme://authority/`: the host (and port) stay for triage,
 *  credentials, path and query never do. */
function redactUrl(match) {
  const schemeEnd = match.indexOf('://');
  const scheme = match.slice(0, schemeEnd + 3);
  const authority = match
    .slice(schemeEnd + 3)
    .split(/[/?#]/)[0]
    .replace(/^.*@/, '');
  return `${scheme}${authority || '[host]'}/`;
}

/** Remove credentials and URL paths/queries, keeping only the authority host. */
export function redactDiagnosticText(value) {
  return String(value ?? '')
    .replace(URLS, redactUrl)
    .replace(BEARER, 'Bearer [redacted]')
    .replace(PROVIDER_KEY, 'sk-[redacted]')
    .replace(SECRET_ASSIGNMENT, match => `${match.split(/[:=]/)[0]}=[redacted]`);
}

function safeFreeText(value) {
  // Limit work before whitespace normalization as well as before redaction.
  const raw = String(value ?? '');
  const window = raw.length > MAX_TEXT * 3 ? `${raw.slice(0, MAX_TEXT * 3)}…` : raw;
  const flattened = window
    .replace(/\s+/g, ' ')
    .trim();
  if (!flattened) return null;
  // Cap before redacting: an arbitrarily long provider message must not make
  // this path cost more than a fixed number of characters.
  const redacted = redactDiagnosticText(flattened);
  return redacted.length > MAX_TEXT ? `${redacted.slice(0, MAX_TEXT)}…` : redacted;
}

function safeToken(value) {
  const raw = String(value ?? '').trim();
  if (!raw || raw.length > MAX_TOKEN || redactDiagnosticText(raw) !== raw) return null;
  // A machine token is a bare word; anything else is prose and does not belong
  // in a structured field.
  return /^[\w.:+-]+$/.test(raw) ? raw : null;
}

function httpStatus(value) {
  const numeric = typeof value === 'number' ? value : Number(value);
  return Number.isInteger(numeric) && numeric >= 100 && numeric <= 599 ? numeric : null;
}

/** The first `file:line:column` in a stack, reduced to its base name. */
function topFrame(stack) {
  // The first line repeats the message, which can be arbitrarily long, so the
  // search window is the next few frames rather than the head of the stack.
  const frames = String(stack ?? '')
    .split('\n')
    .slice(1, 8)
    .join('\n')
    .slice(0, 4096);
  const match = /([A-Za-z0-9_.-]+\.(?:mjs|cjs|js|ts|tsx|jsx)):(\d+):\d+/.exec(frames);
  return match ? `${match[1]}:${match[2]}` : null;
}

/**
 * The whitelisted shape of one error. Never a raw stack, never a request body,
 * and never an unresolved value: an error whose message the worker did not
 * write still only contributes a capped, redacted string here.
 */
export function describeKernelError(error) {
  if (error === null || error === undefined) return null;
  const shape = {
    name: safeToken(error?.name) ?? (typeof error === 'string' ? 'Error' : null),
    message: safeFreeText(typeof error === 'string' ? error : error?.message ?? error?.toString?.()),
    code: safeToken(error?.code ?? error?.cause?.code),
    cause: safeToken(error?.cause?.name) ?? safeFreeText(error?.cause?.message),
    status: httpStatus(error?.status ?? error?.statusCode ?? error?.cause?.status),
    frame: topFrame(error?.stack),
  };
  const evidence = error?.evidence;
  if (evidence && typeof evidence === 'object') {
    // The categorized failure the Host already received; a category is a fixed
    // vocabulary word, so it is safe to echo.
    shape.evidenceCategory = safeToken(evidence.category);
    shape.evidenceHttpStatus = httpStatus(evidence.httpStatus);
  }
  for (const key of Object.keys(shape)) if (shape[key] === null) delete shape[key];
  return Object.keys(shape).length ? shape : null;
}

/** One capped stderr line: `[label] <json>`. Truncated, never unbounded. */
export function diagnosticLine(label, detail) {
  let serialized;
  try {
    serialized = JSON.stringify(detail ?? null);
  } catch {
    serialized = '"[unserializable]"';
  }
  const line = `[${label}] ${serialized}`;
  return line.length > MAX_LINE ? `${line.slice(0, MAX_LINE - 1)}…` : line;
}
