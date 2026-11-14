import { load, saveWorkspace } from "../api";
import type { WorkspaceView } from "../api";

export async function createLocalRules(
  account: string,
  workspace: WorkspaceView,
): Promise<WorkspaceView> {
  const sample = await load();
  return saveWorkspace(account, workspace.revision, {
    documents: sample.sample.documents,
    pins: [],
    as_of: Math.floor(Date.now() / 1000),
    baseline: "current",
  });
}
