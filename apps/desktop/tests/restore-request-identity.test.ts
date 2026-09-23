import { describe, expect, test } from 'bun:test'
import {
  RestoreRequestIdentities,
  restoreActionKey,
} from '../src/features/chat/components/restore-request-identity'

let counter = 0
const mint = () => `restore-${++counter}`

describe('restore request identity follows the action lifecycle', () => {
  test('a resend of an undetermined request reuses the identity (timeout retry)', () => {
    const ids = new RestoreRequestIdentities()
    const key = restoreActionKey('v1', false)
    const first = ids.acquire(key, mint)
    const retry = ids.acquire(key, mint)
    expect(retry).toBe(first)
    expect(ids.size).toBe(1)
  })

  test('a fast double click reuses the identity', () => {
    const ids = new RestoreRequestIdentities()
    const key = restoreActionKey('v2', false)
    const a = ids.acquire(key, mint)
    const b = ids.acquire(key, mint)
    const c = ids.acquire(key, mint)
    expect([a, b, c]).toEqual([a, a, a])
  })

  test('after a definite end the next confirmation is a new action', () => {
    const ids = new RestoreRequestIdentities()
    const key = restoreActionKey('v3', false)
    const first = ids.acquire(key, mint)
    ids.release(key)
    expect(ids.isPending(key)).toBe(false)
    const second = ids.acquire(key, mint)
    expect(second).not.toBe(first)
  })

  test('three consecutive restores of the SAME version get three identities', () => {
    const ids = new RestoreRequestIdentities()
    const key = restoreActionKey('v4', false)
    const seen = [ids.acquire(key, mint)]
    for (let round = 0; round < 2; round += 1) {
      ids.release(key) // the previous restore succeeded
      seen.push(ids.acquire(key, mint))
    }
    expect(new Set(seen).size).toBe(3)
  })

  test('normal turning into force is a different action', () => {
    const ids = new RestoreRequestIdentities()
    const normal = ids.acquire(restoreActionKey('v5', false), mint)
    const forced = ids.acquire(restoreActionKey('v5', true), mint)
    expect(forced).not.toBe(normal)
    expect(ids.size).toBe(2)
  })

  test('different versions are different actions and do not interfere', () => {
    const ids = new RestoreRequestIdentities()
    const a = ids.acquire(restoreActionKey('vA', false), mint)
    const b = ids.acquire(restoreActionKey('vB', false), mint)
    expect(a).not.toBe(b)
    // Releasing one leaves the other undetermined.
    ids.release(restoreActionKey('vA', false))
    expect(ids.isPending(restoreActionKey('vB', false))).toBe(true)
    expect(ids.isPending(restoreActionKey('vA', false))).toBe(false)
  })

  test('a failed request keeps its identity so a retry is deduplicated', () => {
    const ids = new RestoreRequestIdentities()
    const key = restoreActionKey('v6', false)
    const first = ids.acquire(key, mint)
    // The Host refused; the client does not know whether anything was applied,
    // so the request is still undetermined and the identity is kept.
    expect(ids.isPending(key)).toBe(true)
    expect(ids.acquire(key, mint)).toBe(first)
  })
})
