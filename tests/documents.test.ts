/** Exercise the files and image parts received by inference, including a scanned PDF page. */
import { createCanvas, loadImage } from '@napi-rs/canvas';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { Message, Model } from '../src/app/common/schemas/contracts.ts';
import { prepareDocuments } from '../src/app/services/provider-service/documents.ts';
const model: Model = { id: 'vision', name: 'Vision test model', reasoning_efforts: [], default_reasoning_effort: null,
  context_window: 128_000, max_output_tokens: 4_000, tools: true, structured_output: true, input_modalities: ['text', 'image'] };
const fixtures: string = join(import.meta.dirname, 'fixtures');
async function extract(filename: string, mime: string, bytes: Buffer, selected: Model = model): Promise<Message[]> {
  const input: Message[] = [{ role: 'user', content: [
    { type: 'text', text: 'What does this say?' },
    { type: 'file', file: { filename, file_data: `data:${mime};base64,${bytes.toString('base64')}` } },
  ] }];
  return prepareDocuments(input, selected, new AbortController().signal);
}
describe('attachment messages', () => {
  it.each([['txt', 'text/plain'], ['md', 'text/markdown'], ['markdown', 'text/markdown']])(
    'preserves the complete UTF-8 %s separately from the question',
    async (extension, mime): Promise<void> => {
      const text: string = 'Código,Consumo\r\n"Bogotá, D.C.",125\r\n';
      const [attachment, question] = await extract(`flow.${extension}`, mime, Buffer.from(text));
      expect(attachment).toMatchObject({ role: 'user', content: [
        { type: 'text', text: expect.stringContaining(`flow.${extension}`) }, { type: 'text', text },
      ] });
      expect(question).toEqual({ role: 'user', content: [{ type: 'text', text: 'What does this say?' }] });
    },
  );
  it('formats CSV records as a grid with quoted commas, blank fields, and embedded delimiters', async (): Promise<void> => {
    const csv: string = '\uFEFFCódigo,Consumo,Nota\r\n"Bogotá, D.C.",125,"x|y\r\nz"\r\n,,\r\n" ""Sur""",,\r\n';
    const [attachment, question] = await extract('dispatch.csv', 'text/csv', Buffer.from(csv));
    expect(attachment?.content).toEqual([
      { type: 'text', text: expect.stringContaining('dispatch.csv') },
      { type: 'text', text: 'Row | A | B | C\n1 | Código | Consumo | Nota\n2 | Bogotá, D.C. | 125 | "x\\u007cy\\r\\nz"\n3 |  |  | \n4 | " \\"Sur\\"" |  | ' },
    ]);
    expect(question?.content).toEqual([{ type: 'text', text: 'What does this say?' }]);
    await expect(extract('bad.csv', 'text/csv', Buffer.from('one,"unfinished'))).rejects.toMatchObject({ code: 'FILE_UNREADABLE' });
    await expect(extract('big.csv', 'text/csv', Buffer.from('name,value\n'.repeat(20_000)))).rejects.toMatchObject({ code: 'FILE_CONTEXT_LIMIT' });
  }, 15_000);
  it('passes a rendered chart and a rotated scan, including a page without a text layer', async (): Promise<void> => {
    const [attachment] = await extract('report.pdf', 'application/pdf', await readFile(join(fixtures, 'attachment-layout.pdf')));
    expect(attachment?.content).toEqual([
      { type: 'text', text: expect.stringContaining('report.pdf') },
      { type: 'text', text: expect.stringMatching(/Page 1[\s\S]*Dispatch matrix[\s\S]*125 MWh/) },
      { type: 'image_url', image_url: { url: expect.stringMatching(/^data:image\/jpeg;base64,\/9j\//) } },
      { type: 'text', text: 'Page 2\n' },
      { type: 'image_url', image_url: { url: expect.stringMatching(/^data:image\/jpeg;base64,\/9j\//) } },
    ]);
    const parts = attachment?.content;
    if (!Array.isArray(parts)) throw new Error('Expected multipart attachment');
    for (const index of [2, 4]) {
      const { url } = parts[index]?.image_url as { url: string };
      const image = await loadImage(Buffer.from(url.split(',')[1]!, 'base64'));
      const canvas = createCanvas(image.width, image.height);
      const context = canvas.getContext('2d');
      context.drawImage(image, 0, 0);
      const pixels: Uint8ClampedArray = context.getImageData(0, 0, image.width, image.height).data;
      expect(image.width > image.height).toBe(index === 2);
      expect(pixels.some((green: number, offset: number): boolean =>
        offset % 4 === 1 && green > 80 && green > pixels[offset - 1]! + 50 && green > pixels[offset + 1]! + 50)).toBe(true);
    }
  }, 15_000);
  it('keeps the Word table and its embedded image', async (): Promise<void> => {
    const mime: string = 'application/vnd.openxmlformats-officedocument.wordprocessingml.document';
    const [attachment] = await extract('notes.docx', mime, await readFile(join(fixtures, 'attachment-word.docx')));
    expect(attachment?.content).toEqual(expect.arrayContaining([
      { type: 'text', text: expect.stringMatching(/Dispatch notes[\s\S]*North 125 MWh/) },
      { type: 'image_url', image_url: { url: expect.stringMatching(/^data:image\/png;base64,iVBOR/) } },
    ]));
  }, 15_000);
  it('preserves sheet names, sparse cell coordinates and formula expressions', async (): Promise<void> => {
    const mime: string = 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet';
    const [attachment] = await extract('measurements.xlsx', mime, await readFile(join(fixtures, 'attachment-workbook.xlsx')));
    const parts = attachment?.content;
    if (!Array.isArray(parts)) throw new Error('Expected workbook message');
    const text: string = String(parts[1]?.text);
    expect(text.split('\n')).toEqual(expect.arrayContaining([
      'Sheet: Dispatch', 'Row | A | B | C | D | E | F | G',
      '4 |  | 125 |  | 250 |  |  | ', '5 |  | 75 |  |  |  |  | ',
      'D4:D6 (fill down from D4): =B4*2', 'D7: =B7*3', 'D9: =B9*2',
      `E4:E5 (fill down from E4): =IF(B4=$B$4,"B4",'Notes'!A4+LOG10(100))`,
      'F4: =SUM(1:1)', 'F5: =SUM(1:1)', 'G4: =IF(B4>0,"A4",0)', 'G5: =IF(B5>0,"A5",0)',
      'Sheet: Notes', 'Row | A | XFD', '1 | Net generation | ', '20 |  | Far column',
    ]));
    expect(text).not.toContain('\n6 |');
  }, 15_000);
  it('reports corrupt originals and image incompatibility before provider inference', async (): Promise<void> => {
    await expect(extract('report.pdf', 'application/pdf', Buffer.from('not a PDF'))).rejects.toMatchObject({ code: 'FILE_UNREADABLE' });
    await expect(extract('report.pdf', 'application/pdf', Buffer.from('stub'), { ...model, input_modalities: ['text'] })).rejects.toMatchObject({ code: 'FILE_IMAGES_UNSUPPORTED' });
    await expect(extract('bogus.exe', 'text/plain', Buffer.from('example'))).rejects.toMatchObject({ code: 'FILE_TYPE_UNSUPPORTED' });
    await expect(extract('notes.txt', 'text/plain', Buffer.from('a'.repeat(10 * 1024 * 1024 + 1)))).rejects.toMatchObject({ code: 'FILE_REQUEST_LIMIT' });
    await expect(extract('notes.txt', 'text/plain', Buffer.alloc(model.context_window + 1, 97))).rejects.toMatchObject({ code: 'FILE_CONTEXT_LIMIT' });
  }, 15_000);
});
const publicFixtures: string | undefined = process.env.PROVIDER_ROUTER_PUBLIC_FIXTURES;
describe.skipIf(!publicFixtures)('official company documents', () => {
  it.each([
    ['isa-excerpt.pdf', /Estrategia|Sostenibilidad/i, 4],
    ['xm.pdf', /DDV|despacho/i, 14],
    ['epm-excerpt.pdf', /EPM|financiera/i, 4],
    ['ecopetrol-excerpt.pdf', /EBITDA|Ecopetrol/i, 4],
  ])('preserves readable text and each visual page from %s', async (filename, sample, pages): Promise<void> => {
    const [attachment] = await extract(filename, 'application/pdf', await readFile(join(publicFixtures!, filename)));
    expect(JSON.stringify(attachment)).toMatch(sample);
    expect(Array.isArray(attachment?.content) && attachment.content.filter((part): boolean => part.type === 'image_url')).toHaveLength(pages);
  }, 40_000);
  it('rejects a full long PDF cleanly without dropping its remaining pages', async (): Promise<void> => {
    const file: Buffer = await readFile(join(publicFixtures!, 'isa.pdf'));
    await expect(extract('isa.pdf', 'application/pdf', file)).rejects.toMatchObject({ code: 'FILE_CONTEXT_LIMIT' });
  }, 15_000);
  it('extracts the original smaller XM payment workbook as a numbered grid', async (): Promise<void> => {
    const mime: string = 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet';
    const filename: string = 'xm-payment-template.xlsx';
    const [attachment] = await extract(filename, mime, await readFile(join(publicFixtures!, filename)));
    const parts = attachment?.content;
    if (!Array.isArray(parts)) throw new Error('Expected workbook message');
    expect(parts[1]?.text).toMatch(/Sheet: Hoja1\nRow \| A \| B[^\n]*\n2 \| \|? ?.*FORMATO SOLICITUD/);
    expect(parts[1]?.text).toContain('DATOS DEL COMERCIALIZADOR SOLICITANTE');
  }, 15_000);
});
