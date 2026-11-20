// SPDX-License-Identifier: AGPL-3.0-or-later

import type { PDFPageProxy } from 'pdfjs-dist';
import { PasswordException, PDFWorker } from 'pdfjs-dist';
import type { PdfIndexJob, PdfIndexResult } from '../contracts';
import { desktop } from '../desktop';
import { maxImageBytes } from '../editor/rich-content';
import { loadPdf } from './pdf-document';

const maxPageBytes = 256 * 1024;
const maxDocumentBytes = 4 * 1024 * 1024;
const maxPages = 2048;
const documentDeadline = 45_000;
const encoder = new TextEncoder();
type TextContent = Awaited<ReturnType<PDFPageProxy['getTextContent']>>;

async function pageText(page: PDFPageProxy, budget: number) {
  const reader = page.streamTextContent().getReader() as ReadableStreamDefaultReader<TextContent>;
  const parts: string[] = [];
  let bytes = 0;
  let truncated = false;
  try {
    while (!truncated) {
      const { value, done } = await reader.read();
      if (done) break;
      for (const item of value.items) {
        if (!('str' in item)) continue;
        const next = item.str + (item.hasEOL ? '\n' : '');
        if (!next) continue;
        const width = encoder.encode(next).byteLength;
        if (width + bytes > budget) {
          truncated = true;
          await reader.cancel();
          break;
        }
        parts.push(next);
        bytes += width;
      }
    }
  } finally {
    reader.releaseLock();
    page.cleanup();
  }
  const text = parts.join('').replaceAll('\0', ' ').replace(/\s+/gu, ' ').trim();
  return { text, truncated };
}

export async function indexPdf(workspace: string, job: PdfIndexJob, signal: AbortSignal): Promise<PdfIndexResult> {
  const { attachment, generation } = job;
  const result: PdfIndexResult = {
    generation, attachmentId: attachment.id, sha256: attachment.sha256, status: 'unreadable',
    pageCount: null, pages: [],
  };
  if (attachment.bytes > maxImageBytes) return { ...result, status: 'limitReached' };
  let data: ArrayBuffer;
  try {
    data = await desktop.previewAttachment(workspace, attachment.pageId, attachment.id);
  } catch {
    return result;
  }
  if (signal.aborted || !(data instanceof ArrayBuffer)) return result;
  const worker = new PDFWorker();
  const loading = loadPdf(new Uint8Array(data), worker);
  let interrupt = () => {};
  const interrupted = new Promise<void>((resolve) => { interrupt = resolve; });
  let timedOut = false;
  let exceeded = false;
  let pageError = false;
  const deadline = setTimeout(() => { timedOut = true; interrupt(); }, documentDeadline);
  signal.addEventListener('abort', interrupt, { once: true });

  function cancelled() { return signal.aborted || timedOut; }
  async function extract() {
    const document = await loading.promise;
    if (cancelled()) return;
    result.pageCount = document.numPages;
    const last = Math.min(document.numPages, maxPages);
    exceeded = document.numPages > last;
    let remaining = maxDocumentBytes;
    for (let pageNumber = 1; pageNumber <= last && !cancelled(); pageNumber += 1) {
      if (!remaining) { exceeded = true; break; }
      try {
        const part = await pageText(await document.getPage(pageNumber), Math.min(maxPageBytes, remaining));
        if (cancelled()) return;
        exceeded ||= part.truncated;
        if (part.text) {
          result.pages.push({ pageNumber, text: part.text });
          remaining -= encoder.encode(part.text).byteLength;
        }
      } catch {
        pageError = true;
      }
    }
  }
  try {
    await Promise.race([extract(), interrupted]);
    if (result.pages.length) {
      result.status = exceeded || pageError || timedOut ? 'partial' : 'indexed';
    } else if (exceeded || timedOut) {
      result.status = 'limitReached';
    } else if (!pageError && result.pageCount !== null) {
      result.status = 'noText';
    }
  } catch (error) {
    if (timedOut) result.status = 'limitReached';
    else if (error instanceof PasswordException) result.status = 'locked';
  } finally {
    clearTimeout(deadline);
    signal.removeEventListener('abort', interrupt);
    const cleanup = loading.destroy().catch(() => {});
    if (cancelled()) worker.destroy();
    else {
      const close = setTimeout(() => worker.destroy(), 1_000);
      void cleanup.finally(() => { clearTimeout(close); worker.destroy(); });
    }
  }
  return result;
}
