// SPDX-License-Identifier: AGPL-3.0-or-later

import { ZhiyeApiError, ZhiyeClient } from '@zhiye/sdk-ts';

const [action, ...args] = process.argv.slice(2);

async function main() {
  const client = new ZhiyeClient({ baseUrl: process.env.ZHIYE_BASE ?? '', token: process.env.ZHIYE_TOKEN ?? '' });
  if (action === 'search' && args.length === 1) {
    const matches = await client.search({ query: args[0], limit: 10 });
    const first = matches.items[0];
    if (!first) return { matches };
    const page = await client.getPage(first.page_id);
    const content = await client.getPageContent(first.page_id, { format: 'markdown' });
    if (content.etag !== page.etag) throw new Error('读取期间页面发生变化，请重新查询。');
    return { matches, page: page.data, content: content.data };
  }

  if (action === 'propose' && args.length === 4) {
    const [page_id, revision, requestKey, text] = args;
    const base_revision = Number(revision);
    if (!Number.isInteger(base_revision) || base_revision < 1) throw new Error('请使用读取页面时返回的 page_revision。');
    return client.submitProposal({
      page_id, base_revision,
      changes: { operations: [{ op: 'append', block: { type: 'paragraph', payload: { text } } }] },
    }, requestKey);
  }

  throw new Error('用法为 search <关键词> 或 propose <页面 ID> <基准修订> <幂等键> <候选段落>。');
}

try {
  process.stdout.write(JSON.stringify(await main(), null, 2) + '\n');
} catch (error) {
  const report = error instanceof ZhiyeApiError ? error.problem : { message: error.message };
  process.stderr.write(JSON.stringify(report) + '\n');
  process.exitCode = 1;
}
