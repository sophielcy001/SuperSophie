// SPDX-License-Identifier: AGPL-3.0-or-later

import { GlobalWorkerOptions, getDocument } from 'pdfjs-dist';
import type { PDFWorker } from 'pdfjs-dist';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';

GlobalWorkerOptions.workerSrc = workerUrl;
const localData = new URL('/pdf-data/', globalThis.location.href);

export function loadPdf(data: Uint8Array, worker?: PDFWorker) {
  return getDocument({
    data, worker, enableXfa: false, useWasm: false, useWorkerFetch: false,
    disableFontFace: true, maxImageSize: 24_000_000, stopAtErrors: true,
    standardFontDataUrl: new URL('standard_fonts/', localData).href,
    cMapUrl: new URL('cmaps/', localData).href, cMapPacked: true,
  });
}
