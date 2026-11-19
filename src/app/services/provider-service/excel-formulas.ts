/** Describe verified vertical fill ranges while retaining individual formulas for other expressions. */
export interface ExcelFormula { row: number; expression: string }

const tokenPattern: RegExp = /"(?:[^"]|"")*"|'(?:[^']|'')*'|[\p{L}\p{M}\p{N}_.$\\?]+|\s+|./gsu;
const cellPattern: RegExp = /^(\$?)([A-Z]{1,3})(\$?)([1-9]\d{0,6})$/i;

export function* formulaLines(column: string, cells: readonly ExcelFormula[]): IterableIterator<string> {
  let first: ExcelFormula | undefined;
  let last: ExcelFormula | undefined;
  let signature: string | undefined;
  for (const cell of cells) {
    const shape: string | undefined = fillSignature(cell.expression, cell.row);
    if (shape !== undefined && shape === signature && last && cell.row === last.row + 1) {
      last = cell;
      continue;
    }

    if (first && last) yield formatRun(column, first, last);
    first = last = cell;
    signature = shape;
  }

  if (first && last) yield formatRun(column, first, last);
}

function formatRun(column: string, first: ExcelFormula, last: ExcelFormula): string {
  const formula: string = first.expression.startsWith('=') ? first.expression : `=${first.expression}`;
  const text: string = /[\u0000-\u001f\u007f]/u.test(formula) ? JSON.stringify(formula) : formula;
  const start: string = `${column}${first.row}`;
  return first === last ? `${start}: ${text}` : `${start}:${column}${last.row} (fill down from ${start}): ${text}`;
}

/** Relative A1 rows must have the same offset from the formula cell. All other tokens must match exactly. */
function fillSignature(formula: string, row: number): string | undefined {
  const tokens: string[] = formula.match(tokenPattern) ?? [];
  const normalized: (number | string)[] = [];
  for (let index: number = 0; index < tokens.length; index++) {
    const token: string = tokens[index]!;
    const following: string | undefined = nextToken(tokens, index);
    // Keep structured, external, spill and other ambiguous references as individual formulas.
    if (['[', ']', '#', '@', '"', "'"].includes(token)) return undefined;
    if (token === ':' && (!cellReference(tokens[index - 1]) || !cellReference(tokens[index + 1])
      || nextToken(tokens, index + 1) === '!')) return undefined;

    const reference: RegExpExecArray | null = cellReference(token);
    if (!reference || reference[3] || following === '!' || following === '(') {
      normalized.push(token);
      continue;
    }

    normalized.push(reference[1]! + reference[2]!, Number(reference[4]) - row);
  }

  return JSON.stringify(normalized);
}

function nextToken(tokens: string[], index: number): string | undefined {
  const next: string | undefined = tokens[index + 1];
  return next?.trim() === '' ? tokens[index + 2] : next;
}

function cellReference(token?: string): RegExpExecArray | null {
  const reference: RegExpExecArray | null = cellPattern.exec(token ?? '');
  if (!reference || Number(reference[4]) > 1_048_576) return null;
  const column: string = reference[2]!.toUpperCase();
  return column.length === 3 && column > 'XFD' ? null : reference;
}
