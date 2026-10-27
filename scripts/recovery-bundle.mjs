// An independent Node decoder for recovery bundles, so the verification
// scripts check the Rust writer against a second implementation.
// Version 3: MAGIC | sealed header | per file, sealed chunks of at most 1 MiB.
// Versions 1 and 2: one compact JWE (dir, A256GCM) holding the files inline.
import assert from 'node:assert/strict'
import { createDecipheriv } from 'node:crypto'
import { existsSync } from 'node:fs'
import { join } from 'node:path'

const MAGIC = Buffer.from('MAILROOM-RECOVERY\x03', 'latin1')
const CHUNK = 1 << 20

export function bundlePath(directory, id) {
  const stream = join(directory, `${id}.bundle`)
  return existsSync(stream) ? stream : join(directory, `${id}.jwe`)
}

export function isBundleName(name) {
  return /^[0-9a-f]{64}\.(bundle|jwe)$/.test(name)
}

function open(key, nonce, aad, body) {
  const d = createDecipheriv('aes-256-gcm', key, nonce)
  d.setAAD(aad)
  d.setAuthTag(body.subarray(body.length - 16))
  return Buffer.concat([d.update(body.subarray(0, body.length - 16)), d.final()])
}

function legacy(bytes, key) {
  const parts = bytes.toString().split('.')
  const plain = open(key, Buffer.from(parts[2], 'base64url'), Buffer.from(parts[0]), Buffer.concat([Buffer.from(parts[3], 'base64url'), Buffer.from(parts[4], 'base64url')]))
  const decoded = JSON.parse(plain.toString()); plain.fill(0)
  return decoded
}

export function decodeBundle(bytes, key) {
  if (!bytes.subarray(0, MAGIC.length).equals(MAGIC)) return legacy(bytes, key)
  let at = MAGIC.length
  const sealed = () => {
    const len = bytes.readUInt32LE(at)
    const nonce = bytes.subarray(at + 4, at + 16), body = bytes.subarray(at + 16, at + 16 + len)
    assert.equal(body.length, len, 'sealed record is complete')
    at += 16 + len
    return { nonce, body }
  }
  const h = sealed()
  const header = JSON.parse(open(key, h.nonce, Buffer.concat([MAGIC, Buffer.from('\0header')]), h.body).toString())
  assert.equal(header.version, 3)
  const stream = Buffer.from(header.stream, 'hex')
  assert.equal(stream.length, 16)
  const chunkAad = (file, index, last) => {
    const f = Buffer.alloc(4); f.writeUInt32LE(file)
    const i = Buffer.alloc(8); i.writeBigUInt64LE(BigInt(index))
    return Buffer.concat([MAGIC, Buffer.from('\0chunk\0'), stream, f, i, Buffer.from([last ? 1 : 0])])
  }
  let files = 0
  const file = () => {
    const index = files++, parts = []
    for (let chunk = 0; ; chunk++) {
      const { nonce, body } = sealed()
      const full = body.length - 16 === CHUNK
      let plain, last
      if (full) {
        try { plain = open(key, nonce, chunkAad(index, chunk, false), body); last = false } catch { plain = open(key, nonce, chunkAad(index, chunk, true), body); last = true }
      } else { plain = open(key, nonce, chunkAad(index, chunk, true), body); last = true }
      parts.push(plain)
      if (last) return Buffer.concat(parts)
    }
  }
  const body = header.body
  let decoded
  if (body.kind === 'single') decoded = { ...body.captured, ciphertext: file() }
  else {
    assert.equal(body.kind, 'selected')
    decoded = { manifest: body.manifest, accounts: body.accounts.map(a => ({ ...a, ciphertext: file() })), ledger: body.ledger }
  }
  assert.equal(at, bytes.length, 'no bytes after the last file')
  return decoded
}
