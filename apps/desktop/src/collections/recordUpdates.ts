// SPDX-License-Identifier: AGPL-3.0-or-later

import { isTauri } from '@tauri-apps/api/core';
import { createContext, useEffect, useState } from 'react';
import { desktop } from '../desktop';

export const ExternalRecordUpdates = createContext(0);

export function useExternalRecordUpdate(workspace: string | undefined) {
  const [updates, setUpdates] = useState<ReadonlyMap<string, number>>(() => new Map());
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    let frame: number | undefined;
    const workspaces = new Set<string>();
    const subscription = desktop.onWorkspaceContentChanged((id) => {
      if (!active) return;
      workspaces.add(id);
      frame ??= requestAnimationFrame(() => {
        frame = undefined;
        const changed = [...workspaces];
        workspaces.clear();
        setUpdates((previous) => {
          const next = new Map(previous);
          for (const id of changed) next.set(id, (next.get(id) ?? 0) + 1);
          return next;
        });
      });
    }).catch(() => undefined);
    return () => {
      active = false;
      if (frame !== undefined) cancelAnimationFrame(frame);
      void subscription.then((stop) => stop?.());
    };
  }, []);
  return workspace ? updates.get(workspace) ?? 0 : 0;
}
