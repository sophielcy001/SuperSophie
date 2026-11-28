// SPDX-License-Identifier: AGPL-3.0-or-later

import { useContext, useEffect, useState } from 'react';
import type { Collection, CollectionView, RecordPage, ViewRange } from '../contracts';
import { errorMessage } from '../contracts';
import { desktop } from '../desktop';
import { ExternalRecordUpdates } from './recordUpdates';

export function useViewRecords(workspace: string, collection: Collection | undefined, view: CollectionView | undefined,
  offset: number, range: ViewRange | undefined, refresh: string) {
  const [result, setResult] = useState<{ key: string; stable: string; data?: RecordPage; error?: string }>();
  const collectionId = collection?.id;
  const viewId = view?.id;
  const externalUpdate = useContext(ExternalRecordUpdates);
  const key = JSON.stringify([workspace, collectionId, collection?.schemaRevision, viewId, view?.revision, offset, range, refresh, externalUpdate]);
  // While the same records are read again after an edit, the previous ones stay on screen instead of flashing away.
  const stable = JSON.stringify([workspace, collectionId, viewId, offset, range]);
  useEffect(() => {
    if (!viewId || !collectionId) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const read = async () => {
      try {
        const data = await desktop.records(workspace, collectionId, viewId, offset, range);
        if (!active) return;
        setResult({ key, stable, data });
        if (data.records.some((record) => record.computedPending)) timer = setTimeout(() => { void read(); }, 650);
      } catch (error) {
        if (active) setResult({ key, stable, error: errorMessage(error) });
      }
    };
    void read();
    return () => { active = false; clearTimeout(timer); };
  }, [workspace, collectionId, viewId, offset, range, key, stable]);
  if (result?.key === key) return result;
  return result?.stable === stable && result.data ? result : undefined;
}
