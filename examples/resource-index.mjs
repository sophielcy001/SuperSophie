// SPDX-License-Identifier: AGPL-3.0-or-later

export function indexEntry({ kind, id, revision }) {
  return { kind, id, revision };
}

function key({ kind, id }) {
  return `${kind}/${id}`;
}

async function enumerate(client) {
  let page = await client.catalog();
  const cursor = page.change_cursor;
  const items = page.items.map(indexEntry);
  while (page.catalog_cursor) {
    page = await client.catalog({ cursor: page.catalog_cursor });
    items.push(...page.items.map(indexEntry));
  }
  return { cursor, items };
}

export async function replayResourceIndex(client, index) {
  let cursor = index.cursor;
  const resources = new Map(index.items.map((item) => [key(item), item]));
  const seen = new Set();
  const invalidatedDerivedPages = new Set();
  let more;
  do {
    const response = await client.changes(cursor);
    for (const event of response.items) {
      if (seen.has(event.event_id)) continue;
      seen.add(event.event_id);
      if (event.action === 'derived.invalidated') {
        invalidatedDerivedPages.add(event.id);
        continue;
      }
      const old = resources.get(key(event));
      if (old && old.revision > event.revision) continue;
      resources.set(key(event), { ...indexEntry(event), deleted: event.action === 'delete' });
    }
    cursor = response.next_cursor;
    more = response.has_more;
  } while (more);
  return {
    cursor,
    items: [...resources.values()].filter((entry) => !entry.deleted).map(indexEntry),
    invalidatedDerivedPages: [...invalidatedDerivedPages],
    rebuilt: false,
  };
}

export async function refreshResourceIndex(client, last) {
  if (last) {
    try {
      return await replayResourceIndex(client, last);
    } catch (error) {
      if (!['CURSOR_INVALID', 'CURSOR_EXPIRED'].includes(error.problem?.code)) throw error;
    }
  }
  const rebuilt = await replayResourceIndex(client, await enumerate(client));
  return { ...rebuilt, rebuilt: true };
}
