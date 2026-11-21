/** Render keyboard-driven settings, a responsive router dashboard, and masked inputs. */
import { emitKeypressEvents, type Key } from 'node:readline';
import type { ReadStream, WriteStream } from 'node:tty';
import { border, colors, fit, paint, panel, panelRow, plain, shortcut, width, wrap } from './theme.ts';

export interface MenuChoice<T> {
  label: string;
  value: T;
  key?: string;
  description?: string;
}

interface Dashboard {
  endpoint: string | null;
  error: string | null;
  startup: string;
  providers: number;
  exposed: number;
}

interface MenuOptions<T> {
  detail?: readonly string[];
  dashboard?: Dashboard;
  initial?: T;
}

export class TerminalScreen {
  private readonly wasRaw: boolean;

  constructor(
    private readonly input: ReadStream,
    private readonly output: WriteStream,
    private readonly signal: AbortSignal,
    private readonly cancel: () => void,
  ) {
    this.wasRaw = input.isRaw;
    emitKeypressEvents(input);
    input.setRawMode(true);
    input.on('keypress', this.onInterrupt);
    input.resume();
    output.write('\x1b[?1049h\x1b[?25l');
  }

  async ask(title: string, options: { initial?: string; secret?: boolean; hint?: string } = {}): Promise<string | undefined> {
    let value: string = options.initial ?? '';
    const draw: () => void = (): void => {
      const columns: number = this.columns();
      const inside: number = columns - 4;
      const visible: string = options.secret ? '•'.repeat(Math.min(value.length, 64)) : plain(value);
      const field: string = paint(fit(visible + '▏', inside - 1), colors.ink, colors.raised);
      const hint: string[] = options.hint ? wrap(options.hint, inside - 1) : [];
      this.draw([
        border(columns, true),
        panel(title, inside, colors.ink, true),
        ...hint.map((line: string): string => panel(line, inside, colors.muted)),
        panel('', inside),
        panelRow(paint(' ', colors.ink, colors.raised) + field),
        panel('', inside),
        border(columns, false),
      ], [shortcut('Enter', 'Continue'), shortcut('Ctrl+U', 'Clear'), shortcut('Esc', 'Back')], '↵ Continue · Esc Back');
    };
    return this.read((key: Key, typed: string, done: (value?: string) => void): void => {
      if (key.name === 'escape') { done(); return; }
      if (key.name === 'return') { done(value); return; }
      if (key.ctrl && key.name === 'u') value = '';
      else if (key.name === 'backspace') value = [...value].slice(0, -1).join('');
      else if (!key.ctrl && !key.meta) value += plain(typed);
      draw();
    }, draw);
  }

  close(): void {
    this.input.off('keypress', this.onInterrupt);
    this.input.setRawMode(this.wasRaw);
    this.input.pause();
    this.output.write('\x1b[0m\x1b[?25h\x1b[?1049l');
  }

  async menu<T>(title: string, choices: readonly MenuChoice<T>[], options: MenuOptions<T> = {}): Promise<T | undefined> {
    let index: number = Math.max(0, choices.findIndex((choice: MenuChoice<T>): boolean => choice.value === options.initial));
    const draw: () => void = (): void => {
      const columns: number = this.columns();
      const sideBySide: boolean = Boolean(options.dashboard && columns >= 64);
      const menuWidth: number = sideBySide ? Math.floor((columns - 3) * 0.52) : columns;
      const details: string[] = (options.detail ?? []).flatMap((line: string): string[] => wrap(line, menuWidth - 4));
      const compactStatus: string = options.dashboard && !sideBySide
        ? options.dashboard.endpoint ?? options.dashboard.error ?? 'API stopped'
        : '';
      const selected: MenuChoice<T> | undefined = choices[index];
      const description: string[] = selected?.description && this.rows() >= 20 ? wrap(selected.description, menuWidth - 4).slice(0, 2) : [];
      const pageSize: number = Math.max(1, this.rows() - 10 - details.length - (compactStatus ? 1 : 0) - (description.length ? description.length + 1 : 0));
      const start: number = Math.max(0, Math.min(index - Math.floor(pageSize / 2), choices.length - pageSize));
      const rows: string[] = [
        border(menuWidth, true),
        panel(this.menuHeading(title, choices.length, start, pageSize, menuWidth - 4), menuWidth - 4, colors.subtle, true),
        ...details.map((line: string): string => panel(line, menuWidth - 4, colors.muted)),
        ...choices.slice(start, start + pageSize).map((choice: MenuChoice<T>, offset: number): string =>
          this.choiceRow(choice, offset + start === index, menuWidth - 4)),
      ];

      if (description.length) {
        rows.push(panel('', menuWidth - 4));
        for (const line of description) rows.push(panel(line, menuWidth - 4, colors.muted));
      }

      rows.push(border(menuWidth, false));
      const body: string[] = sideBySide && options.dashboard
        ? this.dashboardRows(rows, menuWidth, columns - menuWidth - 3, options.dashboard)
        : [...(compactStatus ? [paint(fit(plain(compactStatus), columns), colors.accent)] : []), ...rows];
      this.draw(body, [
        shortcut('↑ ↓', 'Move'),
        shortcut('Enter', 'Select'),
        shortcut('Esc', options.dashboard ? 'Exit' : 'Back'),
      ], options.dashboard ? '↑↓ Move · ↵ Open · Esc Exit' : '↑↓ Move · ↵ OK · Esc Back');
    };
    return this.read((key: Key, typed: string, done: (value?: T) => void): void => {
      if (key.name === 'escape') { done(); return; }
      if (key.name === 'return') { done(choices[index]?.value); return; }
      if (key.name === 'up') index = Math.max(0, index - 1);
      if (key.name === 'down') index = Math.min(choices.length - 1, index + 1);
      if (key.name === 'home') index = 0;
      if (key.name === 'end') index = Math.max(0, choices.length - 1);
      const matching: MenuChoice<T> | undefined = choices.find((choice: MenuChoice<T>): boolean => choice.key !== undefined && choice.key === typed.toLowerCase());
      if (matching && !key.ctrl && !key.meta) { done(matching.value); return; }
      draw();
    }, draw);
  }

  async message(title: string, lines: readonly string[]): Promise<void> {
    await this.menu(title, [{ label: 'Continue', value: true }], { detail: lines });
  }

  progress(label: string): void {
    const columns: number = this.columns();
    this.draw([
      border(columns, true),
      panel(`●  ${label}`, columns - 4, colors.accent, true),
      border(columns, false),
    ], [shortcut('Ctrl+C', 'Exit')], 'Ctrl+C Exit');
  }

  private choiceRow<T>(choice: MenuChoice<T>, selected: boolean, columns: number): string {
    const background = selected ? colors.raised : colors.panel;
    const label: string = fit(plain(choice.label), columns - 3 - (choice.key ? 3 : 0));
    const pointer: string = selected ? '▌  ' : '   ';
    return panelRow(
      paint(pointer, colors.accent, background, true) +
      paint(label, selected ? colors.ink : colors.muted, background, selected) +
      (choice.key ? paint(` ${choice.key} `, selected ? colors.accent : colors.subtle, background, true) : ''),
    );
  }

  private columns(): number {
    return Math.max(24, Math.min((this.output.columns || 82) - 4, 94));
  }

  private dashboardRows(menu: string[], leftWidth: number, rightWidth: number, info: Dashboard): string[] {
    const inner: number = rightWidth - 4;
    const spacing: string[] = this.rows() >= 20 ? [panel('', inner)] : [];
    const panelLines: string[] = [
      border(rightWidth, true),
      panel('ROUTER STATUS', inner, colors.subtle, true),
      ...spacing,
      panel(info.endpoint ? '●  API running' : '○  API stopped', inner, info.endpoint ? colors.accent : colors.warning, true),
      ...wrap(info.endpoint ?? info.error ?? '', inner).slice(0, 2).map((line: string): string => panel(line, inner, colors.muted)),
      ...spacing,
      panel('Startup behavior', inner, colors.subtle),
      panel(info.startup, inner, colors.ink),
      ...spacing,
      panel(`${info.exposed} enabled · ${info.providers} configured`, inner, colors.muted),
      border(rightWidth, false),
    ];
    const height: number = Math.max(menu.length, panelLines.length);
    menu.splice(-1, 0, ...Array<string>(height - menu.length).fill(panel('', leftWidth - 4)));
    panelLines.splice(-1, 0, ...Array<string>(height - panelLines.length).fill(panel('', inner)));
    return Array.from({ length: height }, (_, index: number): string =>
      menu[index]! + paint('   ') + panelLines[index]!);
  }

  private draw(lines: readonly string[], keys: readonly string[], compactKeys: string): void {
    if ((this.output.columns || 82) < 28 || (this.output.rows || 24) < 8) {
      this.output.write('\x1b[H\x1b[2J' + fit('Resize to continue', this.output.columns || 82));
      return;
    }

    const columns: number = this.columns();
    const rows: number = this.rows();
    const left: number = Math.max(1, Math.floor(((this.output.columns || 82) - columns) / 2));
    const header: string = paint('◈  ', colors.accent, colors.canvas, true) + paint('Provider Router', colors.ink, colors.canvas, true);
    const visible: string[] = [
      '', header, paint('─'.repeat(columns), colors.edge), '',
      ...lines.slice(0, Math.max(1, rows - 7)), '',
    ];
    for (; visible.length < rows - 2;) visible.push('');
    visible.push(columns >= 46 ? keys.join(paint('   ')) : paint(fit(compactKeys, columns), colors.subtle), '');
    const frame: string = visible.slice(0, rows).map((line: string): string => paint(' '.repeat(left)) + line + paint('') + '\x1b[K').join('\r\n');
    this.output.write('\x1b[H' + frame + '\x1b[0m');
  }

  private menuHeading(title: string, total: number, start: number, pageSize: number, columns: number): string {
    const heading: string = plain(title).toUpperCase();
    if (total <= pageSize || columns <= 20) return heading;
    const count: string = `${start + 1}–${Math.min(total, start + pageSize)} / ${total}`;
    return fit(heading, columns - width(count) - 2) + '  ' + count;
  }

  private readonly onInterrupt = (_typed: string, key: Key): void => {
    if (key?.ctrl && key.name === 'c') this.cancel();
  };

  private async read<T>(handle: (key: Key, typed: string, done: (value?: T) => void) => void, draw: () => void): Promise<T | undefined> {
    this.signal.throwIfAborted();
    draw();
    return new Promise<T | undefined>((resolve, reject): void => {
      const cleanup: () => void = (): void => {
        this.input.off('keypress', onKey);
        this.output.off('resize', draw);
        this.signal.removeEventListener('abort', onAbort);
      };
      const onAbort: () => void = (): void => { cleanup(); reject(this.signal.reason); };
      const done: (value?: T) => void = (value?: T): void => { cleanup(); resolve(value); };
      const onKey: (typed: string | undefined, key: Key | undefined) => void = (typed: string | undefined, key: Key | undefined): void => {
        if (!this.signal.aborted && key) handle(key, typed ?? '', done);
      };
      this.input.on('keypress', onKey);
      this.output.on('resize', draw);
      this.signal.addEventListener('abort', onAbort, { once: true });
    });
  }

  private rows(): number {
    return Math.max(8, this.output.rows || 24);
  }
}
