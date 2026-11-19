import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { parseInline, parseMarkdown, plainText, type Block, type Inline } from '../../src/lib/markdown';

/** Every inline run of `blocks`, nested ones included. */
function runs(blocks: Block[]): Inline[] {
  const all: Inline[] = [];
  const walk = (list: Inline[]) => {
    for (const r of list) {
      all.push(r);
      if (r.kind === 'bold' || r.kind === 'link') walk(r.children);
    }
  };
  for (const b of blocks) {
    if (b.kind === 'list') b.items.forEach(walk);
    else walk(b.children);
  }
  return all;
}

describe('the changelog renderer', () => {
  it('reads a release-please entry as headings, a list, bold scopes and links', () => {
    const blocks = parseMarkdown(
      [
        '# Changelog',
        '',
        '## [0.4.3](https://github.com/danielclasen/faderz/compare/v0.4.2...v0.4.3) (2026-11-19)',
        '',
        '',
        '### Features',
        '',
        '* **mix:** balance a stereo channel ([#103](https://github.com/danielclasen/faderz/issues/103)) ([f682151](https://github.com/danielclasen/faderz/commit/f682151))',
        '* a second entry',
        '  that goes on',
      ].join('\n'),
    );
    expect(blocks.map((b) => (b.kind === 'heading' ? `h${b.level}` : b.kind))).toEqual(['h1', 'h2', 'h3', 'list']);
    const release = blocks[1]!;
    expect(release.kind === 'heading' && plainText(release.children)).toBe('0.4.3 (2026-11-19)');
    expect(release.kind === 'heading' && release.children[0]).toMatchObject({ kind: 'link', href: 'https://github.com/danielclasen/faderz/compare/v0.4.2...v0.4.3' });
    const list = blocks[3]!;
    expect(list.kind === 'list' && list.items.map(plainText)).toEqual(['mix: balance a stereo channel (#103) (f682151)', 'a second entry that goes on']);
    expect(list.kind === 'list' && list.items[0]![0]).toEqual({ kind: 'bold', children: [{ kind: 'text', text: 'mix:' }] });
  });

  it('keeps a tag in the text as text and drops a link that is not http or https', () => {
    const inline = parseInline('<img src=x onerror=alert(1)> [click](javascript:alert) [data](data:text/html,x) [ok](https://example.org/a)');
    expect(inline.filter((r) => r.kind === 'link')).toEqual([{ kind: 'link', href: 'https://example.org/a', children: [{ kind: 'text', text: 'ok' }] }]);
    expect(plainText(inline)).toBe('<img src=x onerror=alert(1)> click data ok');
  });

  it('reads inline code and leaves an unclosed marker as text', () => {
    expect(parseInline('a `b` c')).toEqual([
      { kind: 'text', text: 'a ' },
      { kind: 'code', text: 'b' },
      { kind: 'text', text: ' c' },
    ]);
    expect(parseInline('**open and [half](')).toEqual([{ kind: 'text', text: '**open and [half](' }]);
  });

  it('reads all of the repository changelog: every link absolute, none left as markup', () => {
    const blocks = parseMarkdown(readFileSync(new URL('../../../CHANGELOG.md', import.meta.url), 'utf8'));
    expect(blocks.filter((b) => b.kind === 'heading' && b.level === 2).length).toBeGreaterThan(5);
    const all = runs(blocks);
    expect(all.filter((r) => r.kind === 'link').every((r) => r.kind === 'link' && /^https:\/\//.test(r.href))).toBe(true);
    const text = all.filter((r) => r.kind === 'text').map((r) => (r.kind === 'text' ? r.text : ''));
    expect(text.filter((t) => t.includes('**') || /\]\(/.test(t))).toEqual([]);
  });
});
