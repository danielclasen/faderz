// The theme tokens meet WCAG 2.2 AA where an operator reads them: text
// at 4.5:1 on the surfaces it sits on, key edges at 3:1 (1.4.11).

import { readdirSync, readFileSync } from 'node:fs';
import { parse } from 'svelte/compiler';
import { describe, expect, it } from 'vitest';
import { SCRIBBLE_COLORS } from '../../src/lib/contract/types';

const css = readFileSync(new URL('../../src/app.css', import.meta.url), 'utf8');

function tokens(selector: string): Record<string, string> {
  const at = css.indexOf(`${selector} {`);
  const block = css.slice(at, css.indexOf('}', at));
  return Object.fromEntries([...block.matchAll(/--([a-z0-9-]+):\s*(#[0-9a-f]{6})/g)].map((m) => [m[1]!, m[2]!]));
}

function luminance(hex: string): number {
  const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((x) => (x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0]! + 0.7152 * c[1]! + 0.0722 * c[2]!;
}

function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x! + 0.05) / (y! + 0.05);
}

const dark = tokens(':root');
const light = { ...dark, ...tokens(":root[data-theme='light']") };

/** The input rule of NumberEntry.svelte: its text colour and face. */
const entry = (() => {
  const src = readFileSync(new URL('../../src/components/NumberEntry.svelte', import.meta.url), 'utf8');
  const block = /\n  input \{([^}]*)\}/.exec(src)![1]!;
  const prop = (name: string) => new RegExp(`\\n\\s*${name}:\\s*([^;]+);`).exec(block)?.[1]?.trim();
  return { color: prop('color'), background: prop('background') };
})();

/** The colour of the first rule of DuckView.svelte with exactly this selector. */
function duckView(selector: string): string {
  const src = readFileSync(new URL('../../src/components/DuckView.svelte', import.meta.url), 'utf8');
  const at = src.indexOf(`\n  ${selector} {`);
  const block = src.slice(at, src.indexOf('}', at));
  return /\n\s*color:\s*([^;]+);/.exec(block)![1]!.trim();
}

/** A colour as written in a rule: a token of the theme or a literal. */
function resolve(value: string, t: Record<string, string>): string {
  const token = /^var\(--([a-z0-9-]+)\)$/.exec(value);
  return token ? t[token[1]!]! : value;
}

describe.each([
  ['dark', dark],
  ['light', light],
])('%s theme', (_theme, t) => {
  it.each(['strip', 'strip-alt', 'panel', 'panel-2'])('faint text reads on --%s', (surface) => {
    expect(contrast(t['text-faint']!, t[surface]!)).toBeGreaterThanOrEqual(4.5);
  });
  it.each(['panel', 'panel-2', 'strip', 'bg'])('an unlit key edge shows on --%s', (surface) => {
    expect(contrast(t['key-edge']!, t[surface]!)).toBeGreaterThanOrEqual(3);
  });
  it('lit mute, PGM tally and AFL keys carry readable ink', () => {
    expect(contrast(t['mute-ink'] ?? '#ffffff', t['mute']!)).toBeGreaterThanOrEqual(4.5);
    expect(contrast('#ffffff', t['pgm']!)).toBeGreaterThanOrEqual(4.5);
    expect(contrast(t['afl-ink']!, t['afl']!)).toBeGreaterThanOrEqual(4.5);
  });
  it.each(['well-mark', 'well-dim', 'well-duck', 'well-warn', 'well-owned', 'well-afl'])('a mark of --%s shows on a well (3:1)', (mark) => {
    expect(contrast(t[mark]!, t['well']!)).toBeGreaterThanOrEqual(3);
  });
  it('a lit PAUSED loudness key carries readable ink', () => {
    expect(contrast(t['paused-ink']!, t['paused']!)).toBeGreaterThanOrEqual(4.5);
  });
  it('an ownership tag carries readable ink', () => {
    expect(contrast(t['owned-ink']!, t['owned']!)).toBeGreaterThanOrEqual(4.5);
  });
  it('a lit cell of an on/off grid stands apart from an off one', () => {
    expect(contrast(t['on']!, t['switch-off']!)).toBeGreaterThanOrEqual(3);
  });
  it('a typed number entry reads on the field it sits in', () => {
    expect(contrast(resolve(entry.color!, t), resolve(entry.background!, t))).toBeGreaterThanOrEqual(4.5);
  });
  it.each(['thead th', '.setup th', '.idx'])('the Duck view labels of %s read on the page', (rule) => {
    // Every cell of the Duck view table sits on --bg.
    expect(contrast(resolve(duckView(rule), t), t['bg']!)).toBeGreaterThanOrEqual(4.5);
  });
  it('a lit duck key and lamp carry readable ink, and the key edge shows on the panels', () => {
    expect(contrast(t['duck-ink']!, t['duck']!)).toBeGreaterThanOrEqual(4.5);
    for (const surface of ['panel', 'panel-2']) expect(contrast(t['duck']!, t[surface]!)).toBeGreaterThanOrEqual(3);
  });
});

it('leaves a text field the face its component gives it in the light theme', () => {
  // A global light-theme rule on inputs outranks a component's own and
  // painted the fader readouts, which sit on a well, white under light text.
  expect(css).not.toMatch(/:root\[data-theme='light'\][^{]*\binput\b/);
});

it('separates alternate strips in the light theme', () => {
  expect(contrast(light['strip']!, light['strip-alt']!)).toBeGreaterThanOrEqual(1.1);
});

/** color-mix(in srgb, a p%, b) as browsers compute it: per channel, gamma encoded. */
function mix(a: string, p: number, b: string): string {
  const ch = (hex: string, i: number) => parseInt(hex.slice(i, i + 2), 16);
  return `#${[1, 3, 5].map((i) => Math.round(ch(a, i) * (p / 100) + ch(b, i) * (1 - p / 100)).toString(16).padStart(2, '0')).join('')}`;
}

describe('channel colours', () => {
  const strip = readFileSync(new URL('../../src/components/ChannelStrip.svelte', import.meta.url), 'utf8');
  const fader = readFileSync(new URL('../../src/components/Fader.svelte', import.meta.url), 'utf8');
  const colours = [...strip.matchAll(/\.strip\[data-color='([a-z]+)'\] \{ --strip-color: (#[0-9a-f]{6}); \}/g)].map((m) => [m[1]!, m[2]!] as const);
  /** The text colour of the first rule of ChannelStrip.svelte with exactly this selector. */
  const ink = (selector: string) => new RegExp(`\\n  ${selector.replace('.', '\\.')} \\{[^}]*\\n    color: (#[0-9a-f]{6});`).exec(strip)![1]!;
  const inks = ['.head', '.no', '.source'].map((sel) => [sel, ink(sel)] as const);
  const headMix = Number(/var\(--strip-color, var\(--well\)\) (\d+)%, var\(--well\)/.exec(strip)![1]);
  /** --cap-tint of a theme block of app.css. */
  const capTint = (selector: string) => {
    const at = css.indexOf(`${selector} {`);
    return Number(/--cap-tint:\s*(\d+)%/.exec(css.slice(at, css.indexOf('}', at)))![1]);
  };
  const capLine = /\n  \.cap span \{[^}]*background: (#[0-9a-f]{6});/.exec(fader)![1]!;

  it('tints a fader cap by the --cap-tint of its theme', () => {
    expect(fader).toContain('color-mix(in srgb, ${tint} var(--cap-tint), var(--cap))');
  });

  it('has a tint for every colour of the strip colour picker', () => {
    expect(colours.map(([name]) => name)).toEqual(SCRIBBLE_COLORS.filter((c) => c !== 'none'));
  });

  describe.each([
    ['dark', dark, capTint(':root')],
    ['light', light, capTint(":root[data-theme='light']")],
  ])('%s theme', (theme, t, tint) => {
    it.each(colours)('a %s scribble strip stays dark: its name, number and source read', (_name, colour) => {
      const head = mix(colour, headMix, t['well']!);
      for (const [, c] of inks) expect(contrast(c, head)).toBeGreaterThanOrEqual(4.5);
    });
    it('a selected strip shows its --on ring against the strip around it, tinted or not', () => {
      expect(contrast(t['on']!, t['strip-sel']!)).toBeGreaterThanOrEqual(3);
    });
    // Guards the colours, --cap and the line's ink: on these caps no share
    // of the colour would make the line less clear than on a plain one.
    // The mine and foreign rings sit on the strip outside the cap, so a tint
    // never touches what they read against; this pins them at their values
    // before the tint, so a change to the ring or strip tokens shows.
    it('a held cap shows its mine and foreign rings against the strip, tinted or not', () => {
      const ring = (token: string) => {
        const p = Number(new RegExp(`color-mix\\(in srgb, var\\(--${token}\\) (\\d+)%, transparent\\)`).exec(fader)![1]);
        return contrast(mix(t[token]!, p, t['strip']!), t['strip']!);
      };
      const before = theme === 'dark' ? { on: 7.36, owned: 3.15 } : { on: 4.95, owned: 2.87 };
      expect(ring('on')).toBeGreaterThanOrEqual(before.on);
      expect(ring('owned')).toBeGreaterThanOrEqual(before.owned);
    });
    it.each(colours)('a %s fader cap keeps its centre line as clear as a plain one', (_name, colour) => {
      const face = mix(colour, tint, t['cap']!);
      expect(contrast(capLine, face)).toBeGreaterThanOrEqual(Math.min(3, contrast(capLine, t['cap']!)));
    });
  });
});

// A well stays dark in both themes, so text drawn on one takes a colour
// that reads on it in both (a --well-* token or a literal), never a theme
// text token that turns dark in the light theme. An element a rule gives
// the well's background is on the well, and so is everything inside it;
// every text colour a rule gives one of them, short of a background of its
// own, reads at 4.5:1 there. Combinators count as descendant ones. This
// checks text colour only: marks on a well (strokes, fills, outlines) take
// --well-* tokens, which the 3:1 checks above cover. Mixes of the well
// (the scribble strip head) are pinned under channel colours.
describe('text on a well', () => {
  type Node = { type: string; [key: string]: unknown };
  type Element = { name: string; classes: Set<string>; parent: Element | undefined; node: Node };
  /** A compound selector: its type, if any, and its classes. */
  type Compound = { type: string | undefined; classes: string[] };
  type Rule = { selectors: Compound[][]; decls: Record<string, string>; text: string };

  /** The rules of a stylesheet, media blocks included. */
  function* rules(node: unknown, src: string): Generator<Rule> {
    if (Array.isArray(node)) for (const n of node) yield* rules(n, src);
    else if (node && typeof node === 'object') {
      const n = node as Node;
      if (n.type === 'Rule') {
        const prelude = n['prelude'] as Node;
        const selectors = (prelude['children'] as Node[]).map((complex) =>
          (complex['children'] as Node[]).map(({ selectors: s }) => ({
            type: (s as Node[]).find((x) => x.type === 'TypeSelector')?.['name'] as string | undefined,
            classes: (s as Node[]).filter((x) => x.type === 'ClassSelector').map((x) => x['name'] as string),
          })),
        );
        const decls = (n['block'] as { children: Node[] }).children.filter((d) => d.type === 'Declaration');
        yield { selectors, decls: Object.fromEntries(decls.map((d) => [d['property'], d['value']])), text: src.slice(n['start'] as number, prelude['end'] as number) };
      }
      for (const [key, value] of Object.entries(n)) if (key !== 'metadata' && key !== 'prelude') yield* rules(value, src);
    }
  }

  /** The elements of a template, each with its static and directive classes and the element around it. */
  function elements(node: unknown, parent?: Element, out: Element[] = []): Element[] {
    if (Array.isArray(node)) for (const n of node) elements(n, parent, out);
    else if (node && typeof node === 'object') {
      const n = node as Node;
      let inner = parent;
      if (n.type === 'RegularElement') {
        const classes = new Set<string>();
        for (const a of n['attributes'] as Node[]) {
          if (a.type === 'ClassDirective') classes.add(a['name'] as string);
          if (a.type === 'Attribute' && a['name'] === 'class' && Array.isArray(a['value']))
            for (const part of a['value'] as Node[]) if (part.type === 'Text') for (const c of (part['data'] as string).split(/\s+/)) if (c) classes.add(c);
        }
        inner = { name: n['name'] as string, classes, parent, node: n };
        out.push(inner);
      }
      for (const [key, value] of Object.entries(n)) if (key !== 'metadata') elements(value, inner, out);
    }
    return out;
  }

  const fits = (c: Compound, e: Element) => (c.type === undefined || c.type === e.name) && c.classes.every((x) => e.classes.has(x));

  /** Whether a selector can match the element: its subject does, and each compound before it an element around it. */
  function matches(selector: Compound[], e: Element): boolean {
    const subject = selector.at(-1)!;
    if ((subject.type === undefined && subject.classes.length === 0) || !fits(subject, e)) return false;
    let at = e.parent;
    for (const c of selector.slice(0, -1).reverse()) {
      while (at && !fits(c, at)) at = at.parent;
      if (!at) return false;
      at = at.parent;
    }
    return true;
  }

  /** A colour as written in a rule, in a theme: a token, #rgb or #rrggbb; undefined for inherit, transparent or currentColor. */
  function colour(value: string, t: Record<string, string>): string | undefined {
    if (/^(inherit|transparent|currentColor)$/i.test(value)) return undefined;
    const hex = resolve(value, t);
    if (/^#[0-9a-f]{3}$/i.test(hex)) return hex.replace(/[0-9a-f]/gi, '$&$&');
    if (!/^#[0-9a-f]{6}$/i.test(hex)) throw new Error(`cannot resolve ${value}`);
    return hex;
  }

  const isWell = (background: string | undefined) => background === 'var(--well)';
  const dir = new URL('../../src/components/', import.meta.url);

  /** Every rule that colours text on a well: its file, selector and colour. */
  const onWells = readdirSync(dir)
    .filter((f) => f.endsWith('.svelte'))
    .flatMap((file) => {
      const src = readFileSync(new URL(file, dir), 'utf8');
      const ast = parse(src, { modern: true });
      const all = [...rules(ast.css, src)];
      const els = elements(ast.fragment);
      const wells = new Set(els.filter((e) => all.some((r) => (isWell(r.decls['background']) || isWell(r.decls['background-color'])) && r.selectors.some((s) => matches(s, e)))));
      const onWell = (e: Element | undefined): boolean => e !== undefined && (wells.has(e) || onWell(e.parent));
      return all
        .filter((r) => r.decls['color'] !== undefined && [r.decls['background'], r.decls['background-color']].every((b) => b === undefined || b === 'transparent' || isWell(b)))
        .filter((r) => els.some((e) => onWell(e) && r.selectors.some((s) => matches(s, e))))
        .map((r) => ({ at: `${file} ${r.text.trim().replace(/\s+/g, ' ')}`, color: r.decls['color']! }));
    });

  it('finds the text on the wells it guards', () => {
    expect(onWells.map((r) => r.at)).toEqual(expect.arrayContaining(['ConnectionKey.svelte .quorum', 'ConnectionKey.svelte .quorum.degraded', 'VcaMaster.svelte .kind']));
  });

  it.each([
    ['dark', dark],
    ['light', light],
  ])('reads at 4.5:1 on the well in the %s theme', (_theme, t) => {
    const low = onWells.flatMap(({ at, color }) => {
      const c = colour(color, t);
      return c !== undefined && contrast(c, t['well']!) < 4.5 ? [`${at}: ${color} at ${contrast(c, t['well']!).toFixed(2)}:1`] : [];
    });
    expect(low).toEqual([]);
  });
});
