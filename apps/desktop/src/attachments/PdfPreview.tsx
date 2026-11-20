// SPDX-License-Identifier: AGPL-3.0-or-later

import { ChevronLeft, ChevronRight } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { AnnotationMode } from 'pdfjs-dist';
import type { PDFDocumentProxy, RenderTask } from 'pdfjs-dist';
import { loadPdf } from './pdf-document';

export default function PdfPreview({ data, initialPage = 1 }: { data: ArrayBuffer; initialPage?: number }) {
  const [document, setDocument] = useState<PDFDocumentProxy>();
  const [page, setPage] = useState(1);
  const [paintedPage, setPaintedPage] = useState(0);
  const [failure, setFailure] = useState(false);
  const [missingPage, setMissingPage] = useState(false);
  const canvas = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    let active = true;
    const loading = loadPdf(new Uint8Array(data.slice(0)));
    void loading.promise.then((document) => {
      if (!active) return;
      const inRange = initialPage > 0 && initialPage <= document.numPages;
      setMissingPage(!inRange);
      setPage(inRange ? initialPage : 1);
      setDocument(document);
    })
      .catch(() => { if (active) setFailure(true); });
    return () => { active = false; void loading.destroy().catch(() => {}); };
  }, [data, initialPage]);

  useEffect(() => {
    if (!document) return;
    let active = true;
    let rendering: RenderTask | undefined;
    void document.getPage(page).then(async (pdfPage) => {
      if (!active || !canvas.current) return;
      const base = pdfPage.getViewport({ scale: 1 });
      const scale = Math.min(1.6, 1000 / Math.max(1, base.width), 1700 / Math.max(1, base.height));
      const viewport = pdfPage.getViewport({ scale });
      canvas.current.width = Math.max(1, Math.ceil(viewport.width));
      canvas.current.height = Math.max(1, Math.ceil(viewport.height));
      rendering = pdfPage.render({ canvas: canvas.current, viewport, annotationMode: AnnotationMode.DISABLE });
      await rendering.promise;
      if (active) setPaintedPage(page);
    }).catch(() => { if (active) setFailure(true); });
    return () => { active = false; rendering?.cancel(); };
  }, [document, page]);

  if (failure) return <p className="dialog-empty" role="alert">无法预览此 PDF，可以另存原件。</p>;
  return <div className="pdf-preview">
    {missingPage && <p className="dialog-empty" role="status">原件中没有此页码，已打开第一页。</p>}
    {document && <nav aria-label="PDF 翻页" className="pdf-pagination">
      <button type="button" className="icon-button" aria-label="上一页" disabled={page <= 1} onClick={() => setPage(page - 1)}><ChevronLeft size={17} /></button>
      <span>{page} / {document.numPages}</span>
      <button type="button" className="icon-button" aria-label="下一页" disabled={page >= document.numPages} onClick={() => setPage(page + 1)}><ChevronRight size={17} /></button>
    </nav>}
    {paintedPage !== page && <p className="dialog-empty" role="status">正在绘制 PDF…</p>}
    <div className="pdf-canvas"><canvas ref={canvas} aria-label={`PDF 第 ${page} 页`} hidden={paintedPage !== page} /></div>
  </div>;
}
