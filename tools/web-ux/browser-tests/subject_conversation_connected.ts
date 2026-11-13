// 明示選択の実接続driver。起動・隔離はRustの既存ConnectedChatが所有する。
// 外部modelだけがscripted。WSの保存応答を一度落とし、同じ全入力で復元する。
import { assert, assertEquals } from '@std/assert';
import { chromium, type Page } from 'playwright';
import { sha256File } from '../src/artifacts.ts';
import { assertNoPageWideOverflow } from './memory_document_harness.ts';
import { subjectChatHref, subjectHref } from '../../../web/workspace/src/lib/workspace/subjektiv/routes.ts';
const input = JSON.parse(await new Response(Deno.stdin.readable).text());
const origin = new URL(input.origin);
assert(origin.hostname === '127.0.0.1' && origin.protocol === 'http:');
const output = `${input.artifact_dir}/${input.baseline ? 'baseline' : 'messaging'}-${Date.now()}`;
await Deno.mkdir(output, { recursive: true });
const captures: unknown[] = [];
const pageErrors: string[] = [];
const requestErrors: string[] = [];
const commands: Record<string, unknown>[] = [];
let httpChatRequests = 0;
let droppedAck = false;
let dropNextSaved = false;
let delayedSubscription: string | null = null;
let droppedSubscribeAck = false;
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({ viewport: { width: 1440, height: 1000 }, colorScheme: 'light' });
const cookie = (value: string) => ({ name: 'yoi_workspace_session', value, url: origin.origin, httpOnly: true, sameSite: 'Lax' as const });
await context.addCookies([cookie(input.owner_session)]);
const git = async (args: string[]) => { const result = await new Deno.Command('git', { args, stdout: 'piped' }).output(); assert(result.success); return new TextDecoder().decode(result.stdout).trim(); };
const manifest = { evidence: 'real Browser/Backend/SQLite/signed Runtime; external model scripted', source: { commit: await git(['rev-parse', 'HEAD']), tree: await git(['rev-parse', 'HEAD^{tree}']), status: await git(['status', '--porcelain']) }, driverSha256: await sha256File(new URL(import.meta.url).pathname), baseline: input.baseline, visualVerdict: 'UNREVIEWED: author must open images', status: 'running', captures, pageErrors, requestErrors, httpChatRequests: 0, droppedAck: false, messagingCommands: [] as string[] };
const save = () => Deno.writeTextFile(`${output}/review-context.json`, JSON.stringify(manifest, null, 2));
async function capture(page: Page, id: string) {
  await assertNoPageWideOverflow(page);
  const image = `${output}/${id}.png`; await page.screenshot({ path: image, fullPage: true, animations: 'disabled' });
  const aria = `${output}/${id}.aria.txt`; await Deno.writeTextFile(aria, await page.locator('main').ariaSnapshot());
  captures.push({ id, route: new URL(page.url()).pathname, viewport: page.viewportSize(), image, imageSha256: await sha256File(image), aria, alerts: await page.getByRole('alert').allTextContents() }); await save();
}
if (!input.baseline) await context.routeWebSocket('**/protocol/ws', socket => {
  const server = socket.connectToServer();
  socket.onMessage(message => {
    if (typeof message === 'string') { const frame = JSON.parse(message); commands.push(frame); }
    server.send(message);
  });
  server.onMessage(message => {
    if (typeof message === 'string') {
      const frame = JSON.parse(message);
      if (frame.frame === 'messaging_response' && frame.message.result.kind === 'subscribed') {
        if (!droppedSubscribeAck) { droppedSubscribeAck = true; delayedSubscription = message; return; }
        if (delayedSubscription) { socket.send(delayedSubscription); delayedSubscription = null; }
      }
      if (dropNextSaved && frame.frame === 'messaging_response' && frame.message.result.kind === 'saved') { dropNextSaved = false; droppedAck = true; return; }
    }
    socket.send(message);
  });
});
const page = await context.newPage();
page.setDefaultTimeout(30000);
page.on('pageerror', e => pageErrors.push(e.message));
page.on('request', r => { if (/\/chat\/conversations/.test(new URL(r.url()).pathname)) httpChatRequests++; });
page.on('requestfailed', r => { if (!r.failure()?.errorText.includes('ABORTED')) requestErrors.push(new URL(r.url()).pathname); });
try {
  await page.goto(new URL(subjectChatHref(input.workspace_id, input.subject_id), origin).href);
  if (!input.baseline) {
    await page.getByRole('alert').filter({ hasText: /outcome could not be confirmed/ }).waitFor();
    await page.getByText('Conversation connection: disconnected — history is the last confirmed observation', { exact: true }).waitFor();
    await page.getByRole('button', { name: 'Refresh history', exact: true }).click();
  }
  await page.getByText('No public messages yet.', { exact: true }).waitFor();
  await capture(page, 'empty-1440-light');
  if (input.baseline) {
    await page.setViewportSize({ width: 390, height: 900 }); await capture(page, 'empty-390-light');
    manifest.status = 'baseline-captured'; await save(); console.log('baseline captured');
  } else {
    await page.getByRole('button', { name: 'Start conversation body', exact: true }).click();
    await page.getByRole('button', { name: 'Stop conversation body', exact: true }).waitFor();
    const composer = page.getByRole('textbox', { name: `Message to ${input.subject_name}` });
    await composer.fill('WEB_MESSAGING_INPUT');
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    await page.getByText('Message saved; Runtime acceptance is unknown. Retry only this same input.', { exact: true }).waitFor();
    const history = page.getByRole('list', { name: 'Saved public messages' });
    await history.getByText('explicit messaging reply 1', { exact: true }).waitFor();
    assertEquals(await history.getByRole('listitem').count(), 2);
    assert(!(await history.textContent())?.includes('INTERNAL_ASSISTANT'));
    await page.setViewportSize({ width: 390, height: 900 });
    await page.emulateMedia({ colorScheme: 'dark' }); await capture(page, 'saved-delivery-unknown-390-dark');
    dropNextSaved = true;
    await page.getByRole('button', { name: 'Retry same input', exact: true }).click();
    await page.getByRole('alert').filter({ hasText: /outcome could not be confirmed/ }).waitFor();
    assert(droppedAck); await capture(page, 'lost-ack-390-dark');
    await page.reload();
    await page.getByText('Previous send outcome is unknown. Its original input is retained.', { exact: true }).waitFor();
    await page.getByRole('button', { name: 'Retry same input', exact: true }).click();
    await page.getByText('Runtime accepted this input. Processing and replies are not confirmed.', { exact: true }).waitFor();
    assertEquals(await history.getByRole('listitem').count(), 2);
    await page.setViewportSize({ width: 1440, height: 1000 }); await page.emulateMedia({ colorScheme: 'light' }); await capture(page, 'accepted-restored-1440-light');
    await page.setViewportSize({ width: 768, height: 1000 }); await capture(page, 'accepted-restored-768-light');
    const posts = commands.filter(f => f.frame === 'messaging_request' && (f.message as {command: {kind: string}}).command.kind === 'post');
    assertEquals(posts.length, 3);
    const requests = posts.map(f => (f.message as {command: {request: unknown}}).command.request);
    assertEquals(requests[1], requests[0]); assertEquals(requests[2], requests[0]);
    assert(commands.some(f => f.frame === 'request' && (f.message as {method: string}).method === 'subscribe_events'));
    assert(droppedSubscribeAck && delayedSubscription === null);
    assert(commands.some(f => f.frame === 'messaging_request' && (f.message as {command: {kind: string}}).command.kind === 'unsubscribe'), 'late Subscribe ACK must be cleaned up');
    await page.getByRole('button', { name: 'Stop conversation body', exact: true }).click();
    await page.getByRole('button', { name: 'Start conversation body', exact: true }).waitFor();
    await page.getByRole('button', { name: 'Start conversation body', exact: true }).click();
    await page.getByRole('button', { name: 'Stop conversation body', exact: true }).waitFor();
    assertEquals(await history.getByRole('listitem').count(), 2, 'body replacement must retain the same public history');
    assertEquals(httpChatRequests, 0, 'normal conversation must not use HTTP history/polling');
    await page.goto(new URL(subjectChatHref(input.workspace_id, input.other_subject_id), origin).href);
    await page.getByText('No public messages yet.', { exact: true }).waitFor(); assertEquals(await history.getByRole('listitem').count(), 0);
    await page.goto(new URL(subjectChatHref(input.workspace_id, input.subject_id), origin).href);
    await history.getByText('WEB_MESSAGING_INPUT', { exact: true }).waitFor(); assertEquals(await history.getByRole('listitem').count(), 2);
    await context.clearCookies(); await context.addCookies([cookie(input.foreign_session)]);
    await page.reload(); await page.getByRole('alert').first().waitFor();
    assertEquals(await page.getByRole('textbox', { name: `Message to ${input.subject_name}` }).count(), 0);
    assert(!(await page.locator('main').textContent())?.includes('WEB_MESSAGING_INPUT'));
    await capture(page, 'foreign-account-768-light');
    manifest.httpChatRequests = httpChatRequests; manifest.droppedAck = droppedAck;
    manifest.messagingCommands = commands.filter(f => f.frame === 'messaging_request').map(f => (f.message as {command: {kind: string}}).command.kind);
    assertEquals(pageErrors, []); manifest.status = 'passed'; await save();
    console.log('real messaging Browser -> Backend -> Runtime -> explicit public reply passed');
  }
} catch (error) { manifest.status = 'failed'; await capture(page, 'failure').catch(() => {}); await save(); console.error(await page.getByRole('alert').allTextContents()); throw error; }
finally { await browser.close(); }
