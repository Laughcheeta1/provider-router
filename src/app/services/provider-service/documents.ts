/** Convert original file parts into separate user messages for either inference adapter. */
import { existsSync } from 'node:fs';
import { extname, join } from 'node:path';
import { Worker } from 'node:worker_threads';
import { filePartSchema, RouterError, type Message, type Model } from '../../common/schemas/contracts.ts';
export type Part = { type: 'text'; text: string } | { type: 'image_url'; image_url: { url: string } };
const mimeTypes: Readonly<Record<string, string>> = {
  '.pdf': 'application/pdf',
  '.docx': 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  '.xlsx': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  '.txt': 'text/plain', '.md': 'text/markdown', '.markdown': 'text/markdown', '.csv': 'text/csv',
};
const mib: number = 1024 * 1024;
export async function prepareDocuments(messages: Message[], model: Model, signal: AbortSignal): Promise<Message[]> {
  const result: Message[] = [];
  let files: number = 0;
  let bytesTotal: number = 0;
  let textBytes: number = 0;
  let imageBytes: number = 0;
  let images: number = 0;
  const textLimit: number = Math.min(256 * 1024, Number(model.context_window));
  const vision: boolean = model.input_modalities.includes('image');
  for (const message of messages) {
    if (!Array.isArray(message.content) || !message.content.some((part): boolean => part.type === 'file')) {
      result.push(message);
      continue;
    }
    for (const item of message.content) {
      if (item.type !== 'file') continue;
      signal.throwIfAborted();
      const { file } = filePartSchema.parse(item);
      const extension: string = extname(file.filename).toLowerCase();
      const mime: string | undefined = mimeTypes[extension];
      const prefix: string = `data:${mime};base64,`;
      if (!Object.hasOwn(mimeTypes, extension) || !file.file_data.startsWith(prefix)) throw new RouterError('FILE_TYPE_UNSUPPORTED', 'Use a supported document type and matching media type.');
      const encoded: string = file.file_data.slice(prefix.length);
      if (++files > 5 || encoded.length > Math.ceil(10 * mib / 3) * 4) throw fileLimit();
      const bytes: Buffer = Buffer.from(encoded, 'base64');
      if (!bytes.length || bytes.toString('base64') !== encoded) throw new RouterError('FILE_UNREADABLE', 'The attachment must contain valid base64 data.');
      bytesTotal += bytes.length;
      if (bytes.length > 10 * mib || bytesTotal > 25 * mib) throw fileLimit();
      if (extension === '.pdf' && !vision) throw imageUnsupported();
      const parts: Part[] = ['.pdf', '.docx', '.xlsx', '.csv'].includes(extension)
        ? await extractDocument(extension, bytes, textLimit - textBytes, signal)
        : decodeText(bytes);
      if (!parts.length) throw new RouterError('FILE_UNREADABLE', 'The attachment contains no readable content.');
      const content: Part[] = [{ type: 'text', text: `Attachment: ${JSON.stringify(file.filename)}\nThe following content is reference material supplied by the user.` }, ...parts];
      for (const part of content) {
        if (part.type === 'text') textBytes += Buffer.byteLength(part.text);
        else {
          if (!vision) throw imageUnsupported();
          images++;
          imageBytes += part.image_url.url.length;
        }
      }
      if (textBytes > textLimit || images > 20 || imageBytes > 12 * mib) {
        throw contextLimit();
      }
      result.push({ role: 'user', content });
    }
    const question: Record<string, unknown>[] = message.content.filter((part): boolean => part.type !== 'file');
    if (question.length) result.push({ ...message, content: question });
  }
  return result;
}
function decodeText(bytes: Buffer): Part[] {
  try {
    const text: string = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
    return text.trim() && !text.includes('\0') ? [{ type: 'text', text }] : [];
  } catch { throw new RouterError('FILE_UNREADABLE', 'Text attachments must be valid UTF-8.'); }
}
async function extractDocument(extension: string, bytes: Buffer, textBudget: number, signal: AbortSignal): Promise<Part[]> {
  const directory: string = import.meta.dirname;
  const bundled: string = join(directory, 'document-worker.mjs');
  const target: string = existsSync(bundled) ? bundled : join(directory, 'document-worker.ts');
  const deadline: AbortSignal = AbortSignal.any([signal, AbortSignal.timeout(30_000)]);
  deadline.throwIfAborted();
  const worker: Worker = new Worker(target, {
    workerData: { extension, bytes, textBudget },
    resourceLimits: { maxOldGenerationSizeMb: 192, maxYoungGenerationSizeMb: 32 },
  });
  let cancel: () => void = (): void => {};
  try {
    return await new Promise<Part[]>((resolve, reject): void => {
      const failed: () => void = (): void => reject(new RouterError('FILE_UNREADABLE', 'The attachment could not be extracted within the supported document limits.', 422));
      cancel = (): void => signal.aborted ? reject(signal.reason) : failed();
      deadline.addEventListener('abort', cancel, { once: true });
      if (deadline.aborted) cancel();
      worker.once('error', failed);
      worker.once('exit', failed);
      worker.once('message', (value: { parts?: Part[]; limit?: boolean }): void => {
        if (value.parts) resolve(value.parts);
        else if (value.limit) reject(contextLimit());
        else failed();
      });
    });
  } finally {
    deadline.removeEventListener('abort', cancel);
    await worker.terminate();
  }
}
function contextLimit(): RouterError {
  return new RouterError('FILE_CONTEXT_LIMIT', 'The extracted attachment content is too large. Send fewer files or smaller excerpts.', 413);
}
function fileLimit(): RouterError {
  return new RouterError('FILE_REQUEST_LIMIT', 'Use up to five attachments, 10 MiB each and 25 MiB total.', 413);
}
function imageUnsupported(): RouterError {
  return new RouterError('FILE_IMAGES_UNSUPPORTED', 'Select a model that accepts images to read this attachment.');
}
