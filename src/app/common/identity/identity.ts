/** RFC 9562 application identities and independent high-entropy client credentials. */
import { randomBytes } from 'node:crypto';

let lastTimestamp: number = 0;
let sequence: number = 0;
/** Generate UUIDv7 identities without depending on provider-generated identifiers. */
export function uuid7(): string {
  const time: number = Math.max(Date.now(), lastTimestamp);
  sequence = time === lastTimestamp ? (sequence + 1) & 0xfff : randomBytes(2).readUInt16BE() & 0xfff;
  lastTimestamp = time;
  const bytes: Buffer = randomBytes(16);
  bytes.writeUIntBE(time, 0, 6);
  bytes[6] = 0x70 | sequence >>> 8; bytes[7] = sequence & 0xff; bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  const hex: string = bytes.toString('hex');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
