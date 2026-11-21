/** Terminal colors and cell-safe drawing primitives for an original Provider Router theme. */
export const colors = {
  canvas: '#10141B',
  panel: '#171D27',
  raised: '#202B3B',
  edge: '#344257',
  ink: '#E7EEF8',
  muted: '#A9B8CD',
  subtle: '#7E91AC',
  accent: '#75E7D1',
  danger: '#FF999F',
  warning: '#F3C984',
} as const;

type Color = typeof colors[keyof typeof colors];
const segmenter: Intl.Segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
const enableColor: boolean = process.env.NO_COLOR === undefined && process.env.TERM !== 'dumb';
const wide: RegExp = /[\p{Extended_Pictographic}\p{Regional_Indicator}\u1100-\u115f\u2e80-\ua4cf\uac00-\ud7a3\uf900-\ufaff\uff01-\uff60\uffe0-\uffe6\u{20000}-\u{3fffd}]/u;

export function plain(value: string): string {
  return value.replace(/[\u0000-\u001f\u007f-\u009f]/gu, '');
}

export function width(value: string): number {
  let result: number = 0;
  for (const { segment } of segmenter.segment(value)) result += cellWidth(segment);
  return result;
}

export function fit(value: string, columns: number): string {
  if (columns <= 0) return '';
  const valueWidth: number = width(value);
  if (valueWidth <= columns) return value + ' '.repeat(columns - valueWidth);

  let text: string = '';
  let used: number = 0;
  for (const { segment } of segmenter.segment(value)) {
    const size: number = cellWidth(segment);
    if (used + size >= columns) break;
    used += size;
    text += segment;
  }

  return text + '…' + ' '.repeat(columns - used - 1);
}

export function wrap(value: string, columns: number): string[] {
  const lines: string[] = [];
  let line: string = '';
  for (const word of plain(value).split(/\s+/u)) {
    if (!word) continue;
    if (line && width(line + ' ' + word) > columns) { lines.push(line); line = ''; }
    for (const { segment } of segmenter.segment((line ? ' ' : '') + word)) {
      if (width(line) + cellWidth(segment) > columns) { lines.push(line); line = ''; }
      line += segment;
    }
  }

  if (line) lines.push(line);
  return lines;
}

export function paint(value: string, foreground: Color = colors.ink, background: Color = colors.canvas, bold: boolean = false): string {
  if (!enableColor) return value;
  const rgb = (color: Color): string => [1, 3, 5].map((index: number): number => Number.parseInt(color.slice(index, index + 2), 16)).join(';');
  return `\x1b[${bold ? '1' : '22'};38;2;${rgb(foreground)};48;2;${rgb(background)}m${value}`;
}

export function border(columns: number, top: boolean): string {
  return paint((top ? '╭' : '╰') + '─'.repeat(columns - 2) + (top ? '╮' : '╯'), colors.edge);
}

export function panel(content: string, innerColumns: number, foreground: Color = colors.ink, bold: boolean = false): string {
  return panelRow(paint(fit(plain(content), innerColumns), foreground, colors.panel, bold));
}

export function panelRow(content: string): string {
  return paint('│', colors.edge) + paint(' ', colors.ink, colors.panel) + content +
    paint(' ', colors.ink, colors.panel) + paint('│', colors.edge);
}

export function shortcut(key: string, action: string): string {
  return paint(` ${key} `, colors.ink, colors.raised, true) + paint(` ${action}`, colors.subtle);
}

function cellWidth(segment: string): number {
  return /^\p{Mark}+$/u.test(segment) ? 0 : wide.test(segment) ? 2 : 1;
}
