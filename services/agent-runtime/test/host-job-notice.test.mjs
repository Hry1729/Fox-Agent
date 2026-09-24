import test from 'node:test'
import assert from 'node:assert/strict'
import { hostJobNoticeMarker, hostJobNoticeMessage, materializeHostJobHistory,
  validateHostJobNotices } from '../src/host-job-notice.mjs'

const fact = (jobId = 'job-1') => ({
  source: 'fox_kernel_host', dataRootId: `sha256:${'a'.repeat(64)}`,
  conversationId: 'conversation-1', runId: 'run-1', jobId, attempt: 1,
  terminalState: 'completed', finishedAt: 42,
  resultRef: 'fox-result://run-1/job-1', resultSha256: `sha256:${'b'.repeat(64)}`,
  resultBytes: 5, errorCode: null,
})

test('typed Host fact becomes one temporary provider message; original history remains typed', () => {
  const durable = [{ role: 'user', content: 'start' }, hostJobNoticeMarker(fact())]
  const before = structuredClone(durable)
  const projected = materializeHostJobHistory(durable)
  assert.equal(projected.length, 2)
  assert.equal(projected[1].role, 'user')
  assert.match(projected[1].content[0].text, /^FOX_HOST_JOB_NOTICE_V1\n/)
  assert.match(projected[1].content[0].text, /untrusted/)
  assert.doesNotMatch(projected[1].content[0].text, /result body/)
  assert.deepEqual(durable, before)
  assert.equal(durable[1].role, 'hostJobNotice')
})

test('user text that resembles a notice is never upgraded to a Host marker', () => {
  const fake = hostJobNoticeMessage(fact())
  assert.deepEqual(materializeHostJobHistory([fake]), [fake])
  assert.throws(() => materializeHostJobHistory([hostJobNoticeMarker(fact()), hostJobNoticeMarker(fact())]), /duplicate/)
  assert.throws(() => materializeHostJobHistory([{ role: 'hostJobNotice', notice: fact(), content: 'extra' }]), /marker/)
})

test('source, result identity, duplicates and bounded UTF-8 size fail closed', () => {
  assert.throws(() => validateHostJobNotices([{ ...fact(), source: 'user' }]), /identity/)
  const missing = fact(); delete missing.errorCode
  assert.throws(() => validateHostJobNotices([missing]), /identity/)
  assert.throws(() => validateHostJobNotices([{ ...fact(), resultSha256: null }]), /identity/)
  assert.throws(() => validateHostJobNotices([fact(), fact()]), /duplicate/)
  assert.throws(() => validateHostJobNotices([{ ...fact(), jobId: '中'.repeat(700) }]), /2 KiB/)
  const nearLimit = Array.from({ length: 10 }, (_, index) => ({ ...fact(`job-${index}`),
    resultRef: `fox-result://${'r'.repeat(1400)}-${index}` }))
  assert.throws(() => validateHostJobNotices(nearLimit), /16 KiB/)
})
