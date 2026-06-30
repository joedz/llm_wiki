// TS client for the diff overlay feature. Mirrors the
// `diff-overlay.json` shape the dashboard reads, plus helpers
// to refresh and fetch the overlay from the Rust side.

import { invoke } from "@tauri-apps/api/core"

export interface DiffOverlay {
  version: string
  baseBranch: string
  generatedAt: string
  changedFiles: string[]
  changedNodeIds: string[]
  affectedNodeIds: string[]
  warnings: string[]
}

export async function refreshDiffOverlay(
  projectPath: string,
  repoName: string,
  base?: string,
): Promise<DiffOverlay | null> {
  return invoke<DiffOverlay | null>("code_wiki_refresh_diff_overlay", {
    projectPath,
    repoName,
    base,
  })
}

export async function getDiffOverlay(
  projectPath: string,
  repoName: string,
): Promise<DiffOverlay | null> {
  return invoke<DiffOverlay | null>("code_wiki_get_diff_overlay", {
    projectPath,
    repoName,
  })
}

/** Quick helper: is the overlay "interesting" (something to show)? */
export function isOverlayInteresting(o: DiffOverlay | null): boolean {
  if (!o) return false;
  return o.changedNodeIds.length > 0 || o.affectedNodeIds.length > 0;
}
