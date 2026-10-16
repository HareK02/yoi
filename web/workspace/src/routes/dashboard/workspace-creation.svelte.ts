import { goto } from "$app/navigation";
import {
  createOperationKey,
  createWorkspace,
  type CreateWorkspaceRequest,
  creationErrorMessage,
} from "#lib/workspace/api/workspace-catalog.ts";

// 同一actorのsession再確認はformの寿命ではない。これは権限のcacheではなく、
// dashboard内だけに保持する下書き・要求・確定結果。表示/操作/遷移は毎回live sessionで遮断する。
export class WorkspaceCreation {
  open = $state(false);
  displayName = $state("");
  includeRepository = $state(false);
  repositoryKey = $state("");
  repositoryUri = $state("");
  defaultRef = $state("");
  creating = $state(false);
  error = $state<string | null>(null);
  createdWorkspaceId = $state<string | null>(null);
  private lastSubmission:
    | { signature: string; request: CreateWorkspaceRequest }
    | null = null;

  constructor(readonly accountId: string, private canOperate: () => boolean) {}

  async submit(): Promise<void> {
    if (this.creating || this.createdWorkspaceId || !this.canOperate()) return;
    const normalized = {
      display_name: this.displayName.trim(),
      repository: this.includeRepository
        ? {
          repository_key: this.repositoryKey.trim(),
          uri: this.repositoryUri.trim(),
          default_ref: this.defaultRef.trim() || null,
        }
        : null,
    };
    const signature = JSON.stringify(normalized);
    const request = this.lastSubmission?.signature === signature
      ? this.lastSubmission.request
      : { operation_key: createOperationKey(), ...normalized };
    this.lastSubmission = { signature, request };
    this.creating = true;
    this.error = null;
    try {
      const response = await createWorkspace(fetch, request);
      this.createdWorkspaceId = response.workspace.workspace_id;
    } catch (error) {
      this.error = creationErrorMessage(error);
    } finally {
      this.creating = false;
    }
    // 再確認中の成功を匿名/別actorの遷移にしない。同一actor復帰後は確定結果への明示linkを出す。
    if (this.createdWorkspaceId && this.canOperate()) {
      try {
        await goto(`/w/${encodeURIComponent(this.createdWorkspaceId)}`);
      } catch {
        /* 作成成功は確定済み。遷移失敗でも新しい作成要求には変えない。 */
      }
    }
  }
}
