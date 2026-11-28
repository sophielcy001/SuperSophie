// SPDX-License-Identifier: AGPL-3.0-or-later

import { History, Inbox, Plus, Star } from 'lucide-react';
import type { Navigation, PageInfo } from '../contracts';
import { PageIcon } from '../icons';

export function LibraryNavigation({ navigation, pages, selected, busy, onInbox, onOpen }: {
  navigation: Navigation; pages: ReadonlyMap<string, PageInfo>; selected?: string;
  busy: boolean; onInbox(capture: boolean): void; onOpen(id: string): void;
}) {
  function active(ids: string[]) {
    return ids.flatMap((id) => {
      const page = pages.get(id);
      return page && !page.trashRootId ? [page] : [];
    });
  }
  return <nav className="library-nav" aria-label="资料入口">
    <div className="inbox-shortcut">
      <button type="button" className="sidebar-action" aria-label="打开收集箱" disabled={busy} onClick={() => onInbox(false)}>
        <Inbox size={16} aria-hidden />收集箱
      </button>
      <button type="button" className="icon-button" title="快速收集" aria-label="快速收集" disabled={busy} onClick={() => onInbox(true)}>
        <Plus size={17} aria-hidden />
      </button>
    </div>
    <ShortcutList title="收藏" icon={<Star size={15} aria-hidden />} pages={active(navigation.favorites)}
      defaultOpen selected={selected} busy={busy} onOpen={onOpen} />
    <ShortcutList title="最近访问" icon={<History size={15} aria-hidden />} pages={active(navigation.recentPages)}
      selected={selected} busy={busy} onOpen={onOpen} />
  </nav>;
}

function ShortcutList({ title, icon, pages, defaultOpen, selected, busy, onOpen }: {
  title: string; icon: React.ReactNode; pages: PageInfo[]; defaultOpen?: boolean;
  selected?: string; busy: boolean; onOpen(id: string): void;
}) {
  if (!pages.length) return null;
  return <details className="shortcut-group" open={defaultOpen}>
    <summary><span>{icon}{title}</span></summary>
    <div className="shortcut-list">
      {pages.map((page) => <button key={page.id} type="button" className="sidebar-action shortcut-link"
        title={page.title} aria-label={`${title} ${page.title}`} aria-current={page.id === selected ? 'page' : undefined}
        disabled={busy} onClick={() => onOpen(page.id)}>
        <PageIcon kind={page.icon} size={14} /><span>{page.title}</span>
      </button>)}
    </div>
  </details>;
}
