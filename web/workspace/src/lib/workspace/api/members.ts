import type {
  WorkspaceMember,
  WorkspaceMemberAddRequest,
  WorkspaceMemberListResponse,
  WorkspaceMemberMutationResponse,
  WorkspaceMemberRemoveRequest,
} from '#lib/generated/legacy-server-api.ts';
import { loadJson, workspaceApiPath, type ApiResult } from './http.ts';
export type { WorkspaceMember, WorkspaceMemberListResponse, WorkspaceMemberMutationResponse };
const policy = { diagnosticLabel: 'Workspace members', maxResponseBytes: 256 * 1024 };
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid member response');
  return value as Record<string, unknown>;
}
function text(value: unknown): string {
  if (typeof value !== 'string' || !value || value.length > 1024) throw new Error('Invalid member text');
  return value;
}
function nullable(value: unknown): string | null { return value === null ? null : text(value); }
function boolean(value: unknown): boolean {
  if (typeof value !== 'boolean') throw new Error('Invalid member flag');
  return value;
}
export function parseMembers(value: unknown, workspaceId: string): WorkspaceMemberListResponse {
  const v = object(value);
  if (v.workspace_id !== workspaceId || !Array.isArray(v.members) || v.members.length > 200) throw new Error('Invalid member page');
  const members = v.members.map((entry): WorkspaceMember => {
    const m = object(entry);
    const result = { account_id: text(m.account_id), handle: text(m.handle), display_name: text(m.display_name), is_owner: boolean(m.is_owner), membership_key: nullable(m.membership_key) };
    if (result.is_owner !== (result.membership_key === null)) throw new Error('Invalid membership observation');
    return result;
  });
  if (new Set(members.map(m => m.account_id)).size !== members.length) throw new Error('Duplicate members');
  return { workspace_id: workspaceId, membership_change_key: nullable(v.membership_change_key), members, next_after_account_id: nullable(v.next_after_account_id) };
}
function parseMutation(value: unknown, workspaceId: string): WorkspaceMemberMutationResponse {
  const v = object(value);
  if (v.workspace_id !== workspaceId) throw new Error('Wrong member Workspace');
  return { workspace_id: workspaceId, account_id: text(v.account_id), membership_key: nullable(v.membership_key), membership_change_key: text(v.membership_change_key), present: boolean(v.present), replayed: boolean(v.replayed) };
}
export function loadMembers(fetcher: typeof fetch, workspaceId: string, after: string | null = null, signal?: AbortSignal): Promise<ApiResult<WorkspaceMemberListResponse>> {
  const query = new URLSearchParams({ limit: '100' });
  if (after !== null) query.set('after_account_id', after);
  return loadJson(fetcher, `${workspaceApiPath(workspaceId, '/members')}?${query}`, { signal, cache: 'no-store' }, v => parseMembers(v, workspaceId), policy);
}
export function addMember(fetcher: typeof fetch, workspaceId: string, request: WorkspaceMemberAddRequest, signal?: AbortSignal): Promise<ApiResult<WorkspaceMemberMutationResponse>> {
  return loadJson(fetcher, workspaceApiPath(workspaceId, '/members'), { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(request), signal }, v => parseMutation(v, workspaceId), policy);
}
export function removeMember(fetcher: typeof fetch, workspaceId: string, accountId: string, request: WorkspaceMemberRemoveRequest, signal?: AbortSignal): Promise<ApiResult<WorkspaceMemberMutationResponse>> {
  return loadJson(fetcher, workspaceApiPath(workspaceId, `/members/${encodeURIComponent(accountId)}`), { method: 'DELETE', headers: { 'content-type': 'application/json' }, body: JSON.stringify(request), signal }, v => parseMutation(v, workspaceId), policy);
}
