// SPDX-License-Identifier: AGPL-3.0-or-later

(() => {
  if (location.origin !== 'http://tauri.localhost') return;

  const requests = [];
  let indexing = 0;
  const probe = { requests, get indexActive() { return indexing > 0; } };
  window.performanceProbe = probe;
  const send = window.fetch.bind(window);
  const observed = new Set(['read_page', 'search_text', 'advance_search_index']);

  window.fetch = async (input, options) => {
    if (typeof input !== 'string' || options?.method !== 'POST') return send(input, options);
    const url = new URL(input, location.href);
    const command = url.pathname.slice(1);
    if (url.origin !== 'http://ipc.localhost' || !observed.has(command)) return send(input, options);

    const trace = command === 'search_text' ? probe.searchActive : undefined;
    const query = trace && typeof options.body === 'string' ? JSON.parse(options.body)?.request?.query : undefined;
    const start = performance.now();
    const advance = command === 'advance_search_index';
    if (advance) indexing++;
    let response;
    try {
      response = await send(input, options);
    } catch (error) {
      if (advance) indexing--;
      throw error;
    }
    if (response.headers.get('Tauri-Response') !== 'ok'
      || !response.headers.get('Content-Type')?.startsWith('application/json')) {
      if (advance) indexing--;
      return response;
    }

    // Timestamp the reply Tauri decodes on its usual IPC path.
    const decode = response.json.bind(response);
    response.json = async () => {
      try {
        const result = await decode();
        requests.push(command);
        if (trace && trace.query === query) trace.coreRoundTripMs = performance.now() - start;
        if (advance) probe.pending = result.pending;
        return result;
      } finally {
        if (advance) indexing--;
      }
    };
    return response;
  };
})();
