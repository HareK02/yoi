// @vitest-environment happy-dom
import { afterEach, expect, test, vi } from "vitest";
import { refreshPageData } from "./refresh.ts";
import { type DeferredResult, settle } from "./deferred.ts";
const state = vi.hoisted(() => ({ url: new URL("https://example.test/settings"), data: {} as object, invalidateAll: vi.fn(async () => {}) }));
vi.mock("$app/navigation", () => ({ invalidateAll: state.invalidateAll }));
vi.mock("$app/state", () => ({page: state}));
afterEach(() => { vi.clearAllMocks(); });
function gate<T>() { let release!: (value:T)=>void; const promise=new Promise<T>(resolve=>{release=resolve});return{promise,release:(value:T)=>release(value)}; }

test("mutation reload waits for newly published Workspace and page authority", async () => {
  const workspace=gate<DeferredResult<object>>();
  const content=gate<DeferredResult<object>>();
  state.data={deferredPage:settle({old:true})};
  state.invalidateAll.mockImplementation(async()=>{state.data={deferredWorkspace:workspace.promise,deferredPage:content.promise}});
  let complete=false;
  const refresh=refreshPageData().then(()=>{complete=true});
  await Promise.resolve();
  workspace.release({ok:true,data:{}});
  await Promise.resolve();
  expect(complete).toBe(false);
  content.release({ok:true,data:{}});
  await refresh;
  expect(complete).toBe(true);
});

test("denied or superseded reload is not reported as refreshed authority", async () => {
  state.invalidateAll.mockImplementation(async()=>{state.data={deferredPage:Promise.resolve({ok:false,failure:{message:"Denied",status:403,redirect:null}})}});
  await expect(refreshPageData()).rejects.toThrow("Denied");
  const pending=gate<DeferredResult<object>>();
  state.invalidateAll.mockImplementation(async()=>{state.data={deferredPage:pending.promise}});
  const refresh=refreshPageData();
  await Promise.resolve();
  state.data={deferredPage:settle({newer:true})};
  pending.release({ok:true,data:{}});
  await expect(refresh).rejects.toThrow("superseded");
});
