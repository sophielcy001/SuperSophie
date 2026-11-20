// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from 'react';
import type { PageInfo, PdfProgress, SearchIndexStatus } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';

export interface SearchProgress {
  workspace: string;
  pending?: number;
  error?: string;
  refresh: number;
  pdf?: PdfProgress;
  pdfError?: string;
}

export function useSearchIndex(workspace: string | undefined, pages: PageInfo[]) {
  const [progress, setProgress] = useState<SearchProgress>();
  const [pdf, setPdf] = useState<{ workspace: string; progress?: PdfProgress; error?: string }>();
  const [generation, setGeneration] = useState(0);

  useEffect(() => {
    if (!workspace) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;

    async function advance(id: string) {
      try {
        const { pending } = await desktop.advanceSearchIndex(id);
        if (!active) return;
        setProgress((current) => ({
          workspace: id, pending, refresh: (current?.refresh ?? 0) + (pending === 0 ? 1 : 0),
        }));
        if (pending) timer = setTimeout(() => { void advance(id); }, 25);
      } catch (error) {
        if (active) setProgress((current) => ({
          workspace: id, error: errorMessage(error), refresh: current?.refresh ?? 0,
        }));
      }
    }

    void advance(workspace);
    return () => { active = false; clearTimeout(timer); };
  }, [workspace, pages, generation]);

  useEffect(() => {
    if (!workspace) return;
    const controller = new AbortController();
    const { signal } = controller;
    const id = workspace;
    let timer: ReturnType<typeof setTimeout>;

    function report(progress: PdfProgress) {
      if (signal.aborted) return;
      setPdf((current) => current?.workspace === id && !current.error && current.progress
        && current.progress.revision === progress.revision && current.progress.pending === progress.pending
        && current.progress.indexed === progress.indexed && current.progress.partial === progress.partial
        && current.progress.noText === progress.noText && current.progress.failed === progress.failed
        ? current : { workspace: id, progress });
    }

    async function processNext() {
      if (signal.aborted) return;
      let delay = 2500;
      try {
        const { progress, job } = await desktop.pollPdfIndex(id);
        if (signal.aborted) return;
        report(progress);
        if (job) {
          const { indexPdf } = await import('../attachments/pdf-text');
          if (signal.aborted) return;
          const result = await indexPdf(id, job, signal);
          if (signal.aborted) return;
          report(await desktop.finishPdfIndex(id, result));
          delay = 35;
        }
      } catch (error) {
        if (!signal.aborted) setPdf((current) => ({
          workspace: id, progress: current?.workspace === id ? current.progress : undefined,
          error: errorMessage(error),
        }));
      }
      if (!signal.aborted) timer = setTimeout(() => { void processNext(); }, delay);
    }

    void processNext();
    return () => { controller.abort(); clearTimeout(timer); };
  }, [workspace, generation]);

  return {
    progress: workspace ? {
      workspace, refresh: 0, ...(progress?.workspace === workspace ? progress : undefined),
      pdf: pdf?.workspace === workspace ? pdf.progress : undefined,
      pdfError: pdf?.workspace === workspace ? pdf.error : undefined,
    } : undefined,
    restart(index: SearchIndexStatus) {
      if (!workspace) return;
      setProgress((current) => ({
        workspace, pending: index.pending, refresh: (current?.refresh ?? 0) + 1,
      }));
      setPdf({ workspace, progress: index.pdf });
      setGeneration((current) => current + 1);
    },
  };
}
