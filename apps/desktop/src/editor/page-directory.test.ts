import { describe, expect, it } from 'vitest';
import type { PageInfo } from '../contracts';
import { referenceLabel, setPageDirectory } from './page-directory';

const page = (title: string, trashRootId: string | null = null): PageInfo => ({
  id: 'b7a3c1a4-2b7e-4c51-9d7e-0f1d6f0c2a11', parentId: null, title, revision: 1, updatedAt: 1, icon: 'file', trashRootId,
});

describe('page reference labels', () => {
  it('follows renames like a Notion mention but keeps custom link text', () => {
    setPageDirectory([page('无标题')]);
    expect(referenceLabel(page('').id, '无标题')).toBe('无标题');
    setPageDirectory([page('第一版')]);
    expect(referenceLabel(page('').id, '无标题')).toBe('第一版');
    setPageDirectory([page('第二版')]);
    expect(referenceLabel(page('').id, '第一版')).toBe('第二版');
    expect(referenceLabel(page('').id, '下一页')).toBe('下一页');
    expect(referenceLabel('0f3a9f5e-8c8b-4d38-8f43-6d7a1c9e2b10', '别的页面')).toBe('别的页面');
  });
});
