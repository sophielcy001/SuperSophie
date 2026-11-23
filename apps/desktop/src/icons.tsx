// SPDX-License-Identifier: AGPL-3.0-or-later

import { BookOpen, Database, FileText, Folder, Inbox, Sparkles } from 'lucide-react';

export const pageIconOptions = [
  { id: 'file', label: '文档', icon: FileText },
  { id: 'book', label: '书籍', icon: BookOpen },
  { id: 'folder', label: '文件夹', icon: Folder },
  { id: 'inbox', label: '收集箱', icon: Inbox },
  { id: 'sparkles', label: '星光', icon: Sparkles },
  { id: 'database', label: '数据库', icon: Database },
];

const pageIcons = new Map(pageIconOptions.map(({ id, icon }) => [id, icon]));

export function PageIcon({ kind, size = 17 }: { kind: string; size?: number }) {
  const Icon = pageIcons.get(kind) ?? FileText;
  return <Icon size={size} strokeWidth={1.65} aria-hidden="true" />;
}
