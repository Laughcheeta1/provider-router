/** Native usage counters are unsigned 64-bit JSON integers. */
import { z } from 'zod';

export const MAX_U64: bigint = 18_446_744_073_709_551_615n;
const MAX_SAFE_COUNT: bigint = BigInt(Number.MAX_SAFE_INTEGER);
export const tokenCountSchema = z.union([z.number().int().nonnegative(), z.bigint().min(0n).max(MAX_U64)]);
export type TokenCount = z.infer<typeof tokenCountSchema>;

export function tokenCount(value: bigint): TokenCount {
  return value <= MAX_SAFE_COUNT ? Number(value) : value;
}
export function totalTokenCount(input: TokenCount, output: TokenCount): TokenCount {
  const total: bigint = BigInt(input) + BigInt(output);
  return tokenCount(total > MAX_U64 ? MAX_U64 : total);
}

