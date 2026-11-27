/** Bound provider HTTP bodies before materializing JSON. */
import { RouterError } from '../../../common/schemas/contracts.ts';

export const MAX_PROVIDER_BYTES: number = 16_777_216;

export function providerOutputLimit(): RouterError {
  return new RouterError('PROVIDER_OUTPUT_LIMIT', 'The provider response exceeded the supported size.', 502);
}

export async function readProviderBody(response: Response): Promise<Uint8Array> {
  let received: number = 0;
  const body: ReadableStream<Uint8Array> | undefined = response.body?.pipeThrough(new TransformStream({
    transform(chunk: Uint8Array, controller: TransformStreamDefaultController<Uint8Array>): void {
      received += chunk.byteLength;
      if (received > MAX_PROVIDER_BYTES) throw providerOutputLimit();
      controller.enqueue(chunk);
    },
  }));
  return new Uint8Array(await new Response(body).arrayBuffer());
}
