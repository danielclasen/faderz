// The Markdown release-please writes into a CHANGELOG.md, parsed into
// blocks and inline runs that a component renders as elements: ATX
// headings, `*` or `-` list items, paragraphs, and inline links, bold and
// code. Nothing is passed on as HTML: a tag in the text is text, and a
// link that is not http or https is its text alone.

export type Inline =
  | { kind: 'text'; text: string }
  | { kind: 'bold'; children: Inline[] }
  | { kind: 'code'; text: string }
  | { kind: 'link'; href: string; children: Inline[] };

export type Block =
  | { kind: 'heading'; level: 1 | 2 | 3 | 4 | 5 | 6; children: Inline[] }
  | { kind: 'list'; items: Inline[][] }
  | { kind: 'paragraph'; children: Inline[] };

const HEADING = /^(#{1,6})\s+(.*?)\s*#*\s*$/;
const ITEM = /^\s*[*-]\s+(.*)$/;

/** The blocks of `source`; a line indented under a list item continues it. */
export function parseMarkdown(source: string): Block[] {
  const blocks: Block[] = [];
  /** Lines of the paragraph or list item being read. */
  let lines: string[] = [];
  let list: string[][] | null = null;
  const endParagraph = () => {
    if (lines.length > 0) blocks.push({ kind: 'paragraph', children: parseInline(lines.join(' ')) });
    lines = [];
  };
  const endList = () => {
    if (list) blocks.push({ kind: 'list', items: list.map((l) => parseInline(l.join(' '))) });
    list = null;
  };
  for (const raw of source.split(/\r?\n/)) {
    const line = raw.trimEnd();
    if (line.trim() === '') {
      endParagraph();
      endList();
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading) {
      endParagraph();
      endList();
      const level = heading[1]!.length as 1 | 2 | 3 | 4 | 5 | 6;
      blocks.push({ kind: 'heading', level, children: parseInline(heading[2]!) });
      continue;
    }
    const item = ITEM.exec(line);
    if (item) {
      endParagraph();
      (list ??= []).push([item[1]!]);
      continue;
    }
    if (list && /^\s/.test(line)) {
      list.at(-1)!.push(line.trim());
      continue;
    }
    endList();
    lines.push(line.trim());
  }
  endParagraph();
  endList();
  return blocks;
}

/** A link target kept: an absolute http or https URL. */
function safeHref(href: string): string | null {
  try {
    const url = new URL(href);
    return url.protocol === 'http:' || url.protocol === 'https:' ? url.href : null;
  } catch {
    return null;
  }
}

/** The inline runs of `text`: `**bold**`, `` `code` `` and `[text](url)`; anything else is text. */
export function parseInline(text: string): Inline[] {
  const out: Inline[] = [];
  let plain = '';
  const flush = () => {
    if (plain) out.push({ kind: 'text', text: plain });
    plain = '';
  };
  let i = 0;
  while (i < text.length) {
    if (text.startsWith('**', i)) {
      const end = text.indexOf('**', i + 2);
      if (end > i + 2) {
        flush();
        out.push({ kind: 'bold', children: parseInline(text.slice(i + 2, end)) });
        i = end + 2;
        continue;
      }
    }
    if (text[i] === '`') {
      const end = text.indexOf('`', i + 1);
      if (end > i + 1) {
        flush();
        out.push({ kind: 'code', text: text.slice(i + 1, end) });
        i = end + 1;
        continue;
      }
    }
    if (text[i] === '[') {
      const link = /^\[([^\]]+)\]\(([^)\s]+)\)/.exec(text.slice(i));
      if (link) {
        flush();
        const children = parseInline(link[1]!);
        const href = safeHref(link[2]!);
        if (href) out.push({ kind: 'link', href, children });
        else out.push(...children);
        i += link[0].length;
        continue;
      }
    }
    plain += text[i];
    i++;
  }
  flush();
  return out;
}

/** A heading's text without its markup, for a label. */
export function plainText(runs: Inline[]): string {
  return runs.map((r) => (r.kind === 'text' || r.kind === 'code' ? r.text : plainText(r.children))).join('');
}
