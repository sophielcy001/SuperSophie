import { describe, expect, it } from 'vitest';
import desktopPageDraft from '../../../../crates/core/tests/fixtures/desktop-page-draft.json';
import type { Block } from '../contracts';
import { fromBlocks, toBlocks } from './blocks';
import { spreadsheetTable, webAddress } from './rich-content';

const paragraph: Block = {
  id: '29370447-d7a4-4a62-b328-076001422b35',
  type: 'paragraph', schemaVersion: 1, children: [],
  payload: { inline: [
    { type: 'text', text: '中文内容', marks: [{ type: 'bold' }] },
    { type: 'pageReference', pageId: '40806dbc-c4db-4f65-93e2-4104bc4b249f', label: '读书笔记' },
  ] },
};

describe('public block format', () => {
  it('keeps a paragraph identity, marks and internal links while editing', () => {
    const doc = fromBlocks([paragraph]);
    expect(doc.content?.[0]).toMatchObject({
      type: 'paragraph', attrs: { id: paragraph.id },
      content: [
        { type: 'text', text: '中文内容', marks: [{ type: 'bold' }] },
        { type: 'pageReference', attrs: { pageId: '40806dbc-c4db-4f65-93e2-4104bc4b249f', label: '读书笔记' } },
      ],
    });
    expect(toBlocks(doc)).toEqual([paragraph]);
  });

  it('keeps Notion text and background colors, and preserves unknown colors as an unknown block', () => {
    const colored: Block = {
      ...paragraph,
      payload: { inline: [{ type: 'text', text: '重点', marks: [{ type: 'textColor', color: 'red' }, { type: 'backgroundColor', color: 'yellow' }] }] },
    };
    const doc = fromBlocks([colored]);
    expect(doc.content?.[0].content?.[0].marks).toEqual([
      { type: 'textColor', attrs: { color: 'red' } }, { type: 'backgroundColor', attrs: { color: 'yellow' } },
    ]);
    expect(toBlocks(doc)).toEqual([colored]);
    const unknown: Block = { ...paragraph, payload: { inline: [{ type: 'text', text: '重点', marks: [{ type: 'textColor', color: '#ff0000' }] }] } };
    expect(fromBlocks([unknown]).content?.[0].type).toBe('unknownBlock');
  });

  it('keeps a whole block color named as in Notion', () => {
    const tinted: Block = { ...paragraph, payload: { ...paragraph.payload, color: 'blue_background' } };
    const doc = fromBlocks([tinted]);
    expect(doc.content?.[0].attrs?.blockColor).toBe('blue_background');
    expect(toBlocks(doc)).toEqual([tinted]);
    expect(fromBlocks([{ ...paragraph, payload: { ...paragraph.payload, color: 'blue_text' } }]).content?.[0].type).toBe('unknownBlock');
  });

  it('keeps indentation, columns, bookmarks and comments, and rejects malformed ones', () => {
    const block = (type: string, payload: Block['payload'], children: Block[] = []): Block => ({
      id: crypto.randomUUID(), type, schemaVersion: 1, payload, children,
    });
    const line = (text: string, marks?: unknown[]) => block('paragraph', { inline: [{ type: 'text', text, ...(marks ? { marks } : {}) }] });
    const blocks = [
      line('上级', [{ type: 'comment', text: '补充来源' }]),
      { ...line('讨论', [{ type: 'comment', text: '问题', replies: ['回复一', '回复二'] }]), children: [line('子块')] },
      block('columnList', {}, [block('column', { width: 137 }, [line('左')]), block('column', { width: 63 }, [line('右')])]),
      block('bookmark', { url: 'https://example.com/a', title: '示例', description: '摘要' }),
      block('bookmark', { url: 'https://example.com/b', attachmentId: '6f0a5f4e-1a2b-4c3d-8e9f-0a1b2c3d4e5f', mediaType: 'image/png' }),
    ];
    const doc = fromBlocks(blocks);
    // Children of a paragraph show as an indented run after it, as Notion's child blocks.
    expect(doc.content?.slice(0, 6).map((node) => node.type)).toEqual(['paragraph', 'paragraph', 'indentBlock', 'columnList', 'bookmark', 'bookmark']);
    expect(toBlocks(doc)).toEqual(blocks);
    const malformed = [
      block('columnList', {}, [block('column', {}, [line('只有一列')])]),
      block('column', {}, [line('游离的列')]),
      block('columnList', {}, [block('column', { width: 1.5 }, [line('小数宽度')]), block('column', {}, [line('右')])]),
      block('bookmark', { url: 'javascript:alert(1)' }),
      block('bookmark', { url: 'https://example.com', extra: 1 }),
      block('bookmark', { url: 'https://example.com', attachmentId: '6f0a5f4e-1a2b-4c3d-8e9f-0a1b2c3d4e5f' }),
      line('空评论', [{ type: 'comment', text: ' ' }]),
      line('空回复', [{ type: 'comment', text: '问题', replies: [] }]),
    ];
    expect(fromBlocks(malformed).content?.slice(0, 8).map((node) => node.type)).toEqual(Array(8).fill('unknownBlock'));
  });

  it('nests indented blocks under the paragraph or heading above, and keeps a leading indent as a container', () => {
    const line = (text: string): Block => ({ id: crypto.randomUUID(), type: 'paragraph', schemaVersion: 1, payload: { inline: [{ type: 'text', text }] }, children: [] });
    const heading: Block = { id: crypto.randomUUID(), type: 'heading', schemaVersion: 1, payload: { level: 2, inline: [{ type: 'text', text: '标题' }] }, children: [line('一'), { ...line('二'), children: [line('三')] }] };
    const leading: Block = { id: crypto.randomUUID(), type: 'indentBlock', schemaVersion: 1, payload: {}, children: [line('开头缩进')] };
    const doc = fromBlocks([leading, heading]);
    expect(doc.content?.slice(0, 3).map((node) => node.type)).toEqual(['indentBlock', 'heading', 'indentBlock']);
    expect(doc.content?.[2].content?.map((node) => node.type)).toEqual(['paragraph', 'paragraph', 'indentBlock']);
    expect(toBlocks(doc)).toEqual([leading, heading]);
  });

  it('preserves an unknown block with extension metadata on an unrelated edit', () => {
    const future: Block = {
      id: '80a238f1-669d-435c-b437-0d87b0957c94', type: 'futureGrid', schemaVersion: 2,
      payload: { columnWidths: [40, 60] }, children: [], extension: { id: 'future' },
    };
    const doc = fromBlocks([paragraph, future]);
    expect(doc.content?.[1]?.type).toBe('unknownBlock');
    doc.content![0].content![0].text = '编辑后的内容';
    expect(toBlocks(doc)[1]).toEqual(future);
  });

  it('adds an editable paragraph after a document ending in a divider', () => {
    const divider: Block = {
      id: '47b0b6d1-9d0b-4cba-863e-415197950a3a', type: 'horizontalRule', schemaVersion: 1,
      payload: {}, children: [],
    };
    const document = fromBlocks([divider]);
    expect(document.content?.map((node) => node.type)).toEqual(['horizontalRule', 'paragraph']);
    expect(toBlocks(document)).toEqual([divider]);
    document.content![1].content = [{ type: 'text', text: '新增正文' }];
    expect(toBlocks(document)[1].payload.inline).toEqual([{ type: 'text', text: '新增正文' }]);
  });

  it('preserves a stored empty paragraph and omits the editor cursor on an empty page', () => {
    const blank = { ...paragraph, payload: { inline: [] } };
    expect(toBlocks(fromBlocks([blank]))).toEqual([blank]);
    expect(toBlocks(fromBlocks([]))).toEqual([]);
  });

  it('preserves the starting number and style of an ordered list', () => {
    const ordered: Block = {
      id: '611e75d5-ad65-4167-853c-7f5f2dc28e70', type: 'orderedList', schemaVersion: 1,
      payload: { start: 0, type: 'a' },
      children: [{
        id: '9612f34e-a0c8-4777-b96b-99e4ed8a320b', type: 'listItem', schemaVersion: 1, payload: {},
        children: [paragraph],
      }],
    };
    const document = fromBlocks([ordered]);
    expect(document.content?.[0]).toMatchObject({ type: 'orderedList', attrs: { start: 0, type: 'a' } });
    expect(toBlocks(document)).toEqual([ordered]);
  });

  it('preserves the shared desktop draft when editing next to an opaque block', () => {
    const { draft, editedText } = desktopPageDraft;
    const doc = fromBlocks(draft.blocks);
    const [opaque] = draft.blocks[1].children;
    expect(doc.content?.[1]).toMatchObject({
      type: 'callout', content: [{ type: 'unknownBlock', attrs: { id: opaque.id, raw: opaque } }],
    });
    expect(toBlocks(doc)).toEqual(draft.blocks);

    doc.content![0].content![0].text = editedText;
    const expected = structuredClone(draft.blocks);
    expected[0].payload.inline![0].text = editedText;
    expect(toBlocks(doc)).toEqual(expected);
  });

  it('round-trips page content blocks and keeps a nested future shape opaque', () => {
    const make = (type: string, payload: Block['payload'] = {}, children: Block[] = []): Block =>
      ({ id: crypto.randomUUID(), type, schemaVersion: 1, payload, children });
    const file = make('attachmentFile', { attachmentId: crypto.randomUUID() });
    const formula = make('mathBlock', { latex: '\\frac{a}{b}' });
    const future = { ...make('callout', { tone: 'future', additional: true }, [paragraph]), schemaVersion: 2 };
    const fold = make('toggleBlock', { title: '实验', collapsed: true }, [
      make('callout', { tone: 'tip' }, [file, formula, future]),
    ]);
    const outline = make('tableOfContents');
    const document = fromBlocks([fold, outline]);
    expect(document.content?.map((node) => node.type)).toEqual(['toggleBlock', 'tableOfContents', 'paragraph']);
    const [fileNode, formulaNode, futureNode] = document.content![0].content![0].content!;
    expect(fileNode.attrs?.attachmentId).toBe(file.payload.attachmentId);
    expect(formulaNode.attrs?.latex).toBe(formula.payload.latex);
    expect(futureNode.type).toBe('unknownBlock');
    expect(toBlocks(document)).toEqual([fold, outline]);
  });

  it('round-trips image identity, aligned cells, and HTTP link marks', () => {
    const make = (type: string, payload: Block['payload'] = {}, children: Block[] = []): Block =>
      ({ id: crypto.randomUUID(), type, schemaVersion: 1, payload, children });
    const image = make('attachmentImage', { attachmentId: crypto.randomUUID(), mediaType: 'image/png', alt: '照片' });
    const entry = make('paragraph', { inline: [{ type: 'text', text: '文件来源',
      marks: [{ type: 'link', href: 'https://example.test/' }, { type: 'bold' }] }] });
    const table = make('table', {}, [make('tableRow', {}, [
      make('tableHeader', { align: 'center' }, [entry]),
      make('tableHeader', {}, [make('paragraph', { inline: [] })]),
    ])]);
    const editor = fromBlocks([table, image]);
    expect(editor.content?.map((node) => node.type)).toEqual(['table', 'attachmentImage', 'paragraph']);
    expect(toBlocks(editor)).toEqual([table, image]);
  });

  it('keeps a malformed grid opaque and does not convert unsupported URL marks', () => {
    const tsv = spreadsheetTable('甲\t乙\r\n丙')!;
    expect(tsv.content?.[1].content?.[1].content?.[0].content).toEqual([]);
    const table = toBlocks({ type: 'doc', content: [tsv] })[0];
    table.children[1].children.pop();
    const result = fromBlocks([table]);
    expect(result.content?.[0].type).toBe('unknownBlock');
    expect(toBlocks(result)[0]).toEqual(table);
    expect(webAddress('https://example.test/文件')).toBe('https://example.test/%E6%96%87%E4%BB%B6');
    expect(webAddress('file:///tmp/photo.png')).toBeUndefined();
    expect(webAddress('javascript:1')).toBeUndefined();
  });
});
