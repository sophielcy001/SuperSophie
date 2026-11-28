// SPDX-License-Identifier: AGPL-3.0-or-later

import { commentParts } from './page-directory';

/** Comment text with its page mentions as links showing the pages' current titles. */
export function CommentText({ text, onOpenPage }: { text: string; onOpenPage?(id: string): void }) {
  return <>{commentParts(text).map((part, index) => 'text' in part ? <span key={index}>{part.text}</span>
    : onOpenPage ? <button key={index} type="button" className={`comment-mention${part.trashed ? ' is-trashed' : ''}`}
      onMouseDown={(event) => event.preventDefault()} onClick={(event) => { event.stopPropagation(); onOpenPage(part.pageId); }}>
      @{part.label}</button>
      : <span key={index} className={`comment-mention${part.trashed ? ' is-trashed' : ''}`}>@{part.label}</span>)}</>;
}
