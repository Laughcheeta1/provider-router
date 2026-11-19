/** Parse untrusted documents in a cancellable worker without fetching external resources. */
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { parentPort, workerData } from 'node:worker_threads';
import { parse as parseCsv } from 'csv-parse/sync';
import { createCanvas } from '@napi-rs/canvas';
import { parseOffice, OfficeWarningType, type OfficeContentNode, type OfficeParserAST } from 'officeparser';
import { formulaLines, type ExcelFormula } from './excel-formulas.ts';
import type { Part } from './documents.ts';
// RevExt: 1
interface Job { extension: string; bytes: Uint8Array; textBudget: number }
class ExtractionLimit extends Error {}
const job: Job = workerData as Job;
const port = parentPort!;
// RevExt: 2
try {
  const parts: Part[] = await extract(job);
  port.postMessage({ parts });
} catch (error: unknown) {
  port.postMessage({ limit: error instanceof ExtractionLimit });
}  // RevExt: 9
// RevExt: 3
async function extract(input: Job): Promise<Part[]> {
  if (input.extension === '.pdf') return pdf(input);
  if (input.extension === '.csv') return csv(input);
  return office(input);
}  // RevExt: 29
// RevExt: 19
async function pdf({ bytes }: Job): Promise<Part[]> {
  const { getDocument } = await import('pdfjs-dist/legacy/build/pdf.mjs');
  const requireDependency: NodeRequire = createRequire(import.meta.url);
  const root: string = dirname(requireDependency.resolve('pdfjs-dist/package.json'));
  // PDF.js requires directory paths to end in a forward slash on every platform.
  const task = getDocument({ data: bytes, stopAtErrors: true, enableXfa: false, useWorkerFetch: false,
    cMapUrl: join(root, 'cmaps') + '/', standardFontDataUrl: join(root, 'standard_fonts') + '/',
    wasmUrl: join(root, 'wasm') + '/', iccUrl: join(root, 'iccs') + '/', maxImageSize: 16_000_000 });
  try {
    const document = await task.promise;
    if (document.numPages > 20) throw new ExtractionLimit();
    const parts: Part[] = [];
    for (let number: number = 1; number <= document.numPages; number++) {
      const page = await document.getPage(number);
      const extracted = await page.getTextContent();
      const text: string = extracted.items.map((item): string => 'str' in item ? item.str + (item.hasEOL ? '\n' : ' ') : '').join('').trim();
      parts.push({ type: 'text', text: `Page ${number}\n${text}` });
// RevExt: 4
      const size = page.getViewport({ scale: 1 });
      if (!Number.isFinite(size.width) || !Number.isFinite(size.height) || size.width <= 0 || size.height <= 0) {
        throw new Error('Invalid PDF page dimensions');
      }
// RevExt: 17
      const viewport = page.getViewport({ scale: Math.min(2, 1600 / Math.max(size.width, size.height)) });
      const canvas = createCanvas(Math.max(1, Math.ceil(viewport.width)), Math.max(1, Math.ceil(viewport.height)));
      await page.render({ canvas: canvas as unknown as HTMLCanvasElement, viewport }).promise;
      parts.push({ type: 'image_url', image_url: { url: `data:image/jpeg;base64,${(await canvas.encode('jpeg', 82)).toString('base64')}` } });
      page.cleanup();
    }  // RevExt: 37
    return parts;
  } finally { await task.destroy(); }
}  // RevExt: 10
// RevExt: 5
async function office({ extension, bytes, textBudget }: Job): Promise<Part[]> {
  const document: OfficeParserAST = await parseOffice(Buffer.from(bytes), {
    fileType: extension === '.docx' ? 'docx' : 'xlsx', allowRemoteAssets: false,
    extractAttachments: true, decompressionLimits: { maxUncompressedBytes: 64 * 1024 * 1024, maxZipEntries: 2048 },
  });  // RevExt: 42
  if (document.warnings.some((issue): boolean =>
    issue.code === OfficeWarningType.TABLE_CELL_LIMIT_EXCEEDED ||
    issue.code === OfficeWarningType.TABLE_GRID_LIMIT_EXCEEDED ||
    issue.code === OfficeWarningType.REPEATED_CONTENT_LIMIT_EXCEEDED)) throw new ExtractionLimit();
// RevExt: 18
  const text: string = extension === '.xlsx'
    ? workbookText(document, textBudget)
    : (await document.to('text')).value;
  const parts: Part[] = text.trim() ? [{ type: 'text', text }] : [];
  for (const attachment of document.attachments ?? []) {
    if (attachment.type !== 'image') continue;
    if (!['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(attachment.mimeType) || !attachment.data) throw new Error('Unsupported embedded image');
    parts.push({ type: 'text', text: `Embedded image: ${attachment.name}` });
    parts.push({ type: 'image_url', image_url: { url: `data:${attachment.mimeType};base64,${attachment.data}` } });
  }  // RevExt: 14
  return parts;
}  // RevExt: 11
// RevExt: 6
function* sheetText(sheet: Extract<OfficeContentNode, { type: 'sheet' }>): IterableIterator<string> {
  yield `Sheet: ${inlineText(sheet.metadata?.sheetName ?? '')}`;
  const values: Map<number, Map<number, string>> = new Map();
  const formulas: Map<number, ExcelFormula[]> = new Map();
  const occupied: Set<number> = new Set();
  for (const row of sheet.children ?? []) {
    if (row.type !== 'row') continue;
    for (const cell of row.children ?? []) {
      if (cell.type !== 'cell' || !cell.metadata || (!cell.text && !cell.metadata.formula)) continue;
      const { row: index, col, formula } = cell.metadata;
      occupied.add(col);
      if (cell.text) mapEntry(values, index + 1, () => new Map<number, string>()).set(col, cell.text);
      if (formula) mapEntry(formulas, col, (): ExcelFormula[] => []).push({ row: index + 1, expression: formula });
    }  // RevExt: 38
// RevExt: 7
  }  // RevExt: 15
  const used: number[] = [...occupied].sort((a: number, b: number): number => a - b);
  // Retain blank columns in ordinary tables. Distant sparse cells keep their letters without thousands of blanks.
  const columns: number[] = (used.at(-1) ?? 32) < 32 ? Array.from({ length: used.at(-1)! + 1 }, (_, i: number): number => i) : used;
  if (columns.length) {
    yield gridHeader(columns);
    for (const [number, row] of [...values].sort(([a], [b]): number => a - b)) {
      yield gridRow(number, columns.map((column: number): string => row.get(column) ?? ''));
    }  // RevExt: 39
  }  // RevExt: 44
// RevExt: 20
  if (formulas.size) {
    yield 'Formulas (shown separately; rows without stored values appear only here)';
    for (const column of used) {
      const cells: ExcelFormula[] | undefined = formulas.get(column);
      if (cells) yield* formulaLines(columnName(column), cells.sort((a, b): number => a.row - b.row));
    }  // RevExt: 40
  }  // RevExt: 45
}  // RevExt: 12
// RevExt: 21
function workbookText(document: OfficeParserAST, textBudget: number): string {
  function* lines(): IterableIterator<string> {  // RevExt: 49
    for (const sheet of document.content) {
      if (sheet.type === 'sheet') yield* sheetText(sheet);
    }  // RevExt: 41
  }  // RevExt: 46
  return boundedLines(lines(), textBudget);
}  // RevExt: 30
// RevExt: 22
function csv({ bytes, textBudget }: Job): Part[] {
  const text: string = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  if (text.includes('\0')) throw new Error('Invalid CSV text');
  if (!text.trim()) return [];
  let width: number = 0;
  let minimumBytes: number = 0;
  const records: string[][] = parseCsv(text, {
    relax_column_count: true,
    on_record: (record: string[]): string[] => {
      width = Math.max(width, record.length);
      minimumBytes += 1 + record.length * 3 + record.reduce((sum: number, cell: string): number => sum + Buffer.byteLength(cell), 0);
      if (minimumBytes > textBudget) throw new ExtractionLimit();
      return record;
    },
  });  // RevExt: 43
// RevExt: 23
  function* lines(): IterableIterator<string> {  // RevExt: 50
    yield gridHeader(Array.from({ length: width }, (_, index: number): number => index));
    for (const [index, record] of records.entries()) yield gridRow(index + 1, record);
  }  // RevExt: 47
  return records.length ? [{ type: 'text', text: boundedLines(lines(), textBudget) }] : [];
}  // RevExt: 31
// RevExt: 24
function boundedLines(lines: Iterable<string>, textBudget: number): string {
  const result: string[] = [];
  let bytes: number = 0;
  for (const line of lines) {
    bytes += Buffer.byteLength(line) + Number(result.length > 0);
    if (bytes > textBudget) throw new ExtractionLimit();
    result.push(line);
  }  // RevExt: 48
  return result.join('\n');
}  // RevExt: 32
// RevExt: 25
function gridHeader(columns: number[]): string {
  return ['Row', ...columns.map(columnName)].join(' | ');
}  // RevExt: 33
// RevExt: 26
function gridRow(row: number, cells: string[]): string {
  return [String(row), ...cells.map(inlineText)].join(' | ');
}  // RevExt: 34
// RevExt: 27
function inlineText(value: string): string {
  // Quoted JSON escapes keep a literal pipe, newline or leading whitespace inside one cell.
  return /[\u0000-\u001f\u007f|\\"]|^\s|\s$/u.test(value) ? JSON.stringify(value).replaceAll('|', '\\u007c') : value;
}  // RevExt: 35
// RevExt: 28
function mapEntry<Key, Value>(map: Map<Key, Value>, key: Key, create: () => Value): Value {
  const existing: Value | undefined = map.get(key);
  if (existing !== undefined) return existing;
  const value: Value = create();
  map.set(key, value);
  return value;
}  // RevExt: 36
// RevExt: 8
function columnName(index: number): string {
  let name: string = '';
  for (let column: number = index + 1; column > 0; column = Math.floor((column - 1) / 26)) {
    name = String.fromCharCode(65 + (column - 1) % 26) + name;
  }  // RevExt: 16
  return name;
}  // RevExt: 13
