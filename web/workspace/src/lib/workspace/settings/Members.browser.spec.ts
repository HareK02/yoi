// @vitest-environment happy-dom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, expect, test, vi } from 'vitest';
import Members from './Members.svelte';
import { SESSION_CHANGED_EVENT } from '#lib/workspace/auth/hosting.ts';
const owner = { account_id: 'owner-account', handle: 'owner', display_name: 'Workspace owner', is_owner: true, membership_key: null };
const member = { account_id: 'member-account', handle: 'alice', display_name: 'Alice Example', is_owner: false, membership_key: 'membership-original' };
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
function boundary(options: { listStatus?: number; saveStatus?: number; unknown?: boolean } = {}) {
  let members = [owner, member];
  let changeKey = 'change-original';
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async (input, init) => {
    const path = String(input);
    if (init?.method === 'POST' || init?.method === 'DELETE') {
      if (options.unknown) throw new TypeError('offline');
      if (options.saveStatus) return new Response(null, { status: options.saveStatus });
      if (init.method === 'POST') members = [...members, { ...member, account_id: 'bob-account', handle: 'bob', display_name: 'Bob Example', membership_key: 'membership-bob' }];
      else members = members.filter(m => m.account_id !== 'member-account');
      changeKey = 'change-new';
      return Response.json({ workspace_id: 'workspace-a', account_id: init.method === 'POST' ? 'bob-account' : 'member-account', membership_key: init.method === 'POST' ? 'membership-bob' : null, membership_change_key: changeKey, present: init.method === 'POST', replayed: false });
    }
    if (!path.startsWith('/api/w/workspace-a/members')) throw new Error(`Unexpected path ${path}`);
    if (options.listStatus) return new Response(null, { status: options.listStatus });
    return Response.json({ workspace_id: 'workspace-a', membership_change_key: changeKey, members, next_after_account_id: null });
  });
  vi.stubGlobal('fetch', fetcher);
  return fetcher;
}
test('owner can directly add by account ID, then sees fresh identity and removes with observed key', async () => {
  const fetcher = boundary();
  render(Members, { workspaceId: 'workspace-a' });
  expect(await screen.findByText('Alice Example')).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Remove @owner' })).toBeNull();
  expect(screen.queryByRole('textbox')).toBeNull();
  await fireEvent.click(screen.getByRole('button', { name: 'Add member' }));
  const input = screen.getByLabelText('Account ID (handle)');
  await waitFor(() => expect(document.activeElement).toBe(input));
  await fireEvent.input(input, { target: { value: 'bob' } });
  await fireEvent.click(screen.getByRole('button', { name: 'Add member' }));
  await screen.findByText('Bob Example');
  const add = fetcher.mock.calls.find(([, init]) => init?.method === 'POST')!;
  expect(JSON.parse(String(add[1]?.body))).toMatchObject({ account_handle: 'bob', expected_membership_change_key: 'change-original' });
  await fireEvent.click(screen.getByRole('button', { name: 'Remove @alice' }));
  await waitFor(() => expect(screen.queryByText('Alice Example')).toBeNull());
  expect(await screen.findByText('Bob Example')).toBeTruthy();
  const remove = fetcher.mock.calls.find(([, init]) => init?.method === 'DELETE')!;
  expect(remove[0]).toBe('/api/w/workspace-a/members/member-account');
  expect(JSON.parse(String(remove[1]?.body)).expected_membership_key).toBe('membership-original');
});
for (const status of [401, 403, 404]) test(`HTTP ${status} hides privileged member controls`, async () => {
  boundary({ listStatus: status }); render(Members, { workspaceId: 'workspace-a' });
  await screen.findByRole('alert');
  expect(screen.queryByRole('button', { name: 'Add member' })).toBeNull();
  expect(screen.queryByText('Alice Example')).toBeNull();
});
for (const options of [{ saveStatus: 409 }, { unknown: true }]) test(`failed/unknown removal preserves list and blocks further mutation until reread ${JSON.stringify(options)}`, async () => {
  const fetcher = boundary(options); render(Members, { workspaceId: 'workspace-a' });
  await screen.findByText('Alice Example');
  await fireEvent.click(screen.getByRole('button', { name: 'Remove @alice' }));
  await screen.findByRole('alert');
  expect(screen.getByText('Alice Example')).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Remove @alice' }) as HTMLButtonElement).disabled).toBe(true);
  expect(fetcher.mock.calls.filter(([, init]) => init?.method === 'DELETE')).toHaveLength(1);
  await fireEvent.click(screen.getByRole('button', { name: 'Reload members' }));
  await waitFor(() => expect((screen.getByRole('button', { name: 'Remove @alice' }) as HTMLButtonElement).disabled).toBe(false));
});
test('session change clears previously loaded member data on authorization loss', async () => {
  const options = { listStatus: 0 };
  boundary(options); render(Members, { workspaceId: 'workspace-a' });
  await screen.findByText('Alice Example');
  options.listStatus = 403;
  window.dispatchEvent(new Event(SESSION_CHANGED_EVENT));
  await screen.findByRole('alert');
  expect(screen.queryByText('Alice Example')).toBeNull();
});
for (const changed of [false, true]) test(`pagination ${changed ? 'blocks stale list' : 'appends members'} without mixing observations`, async () => {
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async input => {
    const next = String(input).includes('after_account_id=owner-account');
    return Response.json({ workspace_id: 'workspace-a', membership_change_key: next && changed ? 'concurrent-change' : 'same-change', members: next ? [member] : [owner], next_after_account_id: next ? null : owner.account_id });
  });
  vi.stubGlobal('fetch', fetcher);
  render(Members, { workspaceId: 'workspace-a' });
  await screen.findByText('Workspace owner');
  await fireEvent.click(screen.getByRole('button', { name: 'Load more members' }));
  if (changed) {
    await screen.findByRole('alert');
    expect(screen.queryByText('Alice Example')).toBeNull();
    expect((screen.getByRole('button', { name: 'Add member' }) as HTMLButtonElement).disabled).toBe(true);
  } else {
    await screen.findByText('Alice Example');
    expect(screen.getByText('Workspace owner')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Load more members' })).toBeNull();
  }
});
test('late response from previous Workspace cannot replace current denied state', async () => {
  let resolve!: (response: Response) => void;
  vi.stubGlobal('fetch', vi.fn<typeof fetch>().mockImplementation(async input => String(input).includes('/workspace-a/') ? new Promise(r => { resolve = r; }) : new Response(null, { status: 403 })));
  const view = render(Members, { workspaceId: 'workspace-a' });
  await waitFor(() => expect(resolve).toBeDefined());
  await view.rerender({ workspaceId: 'workspace-b' });
  await screen.findByRole('alert');
  resolve(Response.json({ workspace_id: 'workspace-a', membership_change_key: null, members: [owner], next_after_account_id: null }));
  await new Promise(r => setTimeout(r, 0));
  expect(screen.queryByText('Workspace owner')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Add member' })).toBeNull();
});
