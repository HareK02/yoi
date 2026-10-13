import { invalidateAll } from "$app/navigation";
import { page } from "$app/state";
import { type DeferredResult, resolvePageData } from "./deferred.ts";

// mutation後のauthority再取得は、RouterがPromiseを差し替えた時点では完了していない。
// 保持中のcontent propsではなく、新しく公開されたroute dataの応答まで待つ。
export async function refreshPageData(): Promise<void> {
  const url = page.url.href;
  await invalidateAll();
  if (page.url.href !== url) throw new Error("Page changed while refreshing");
  const data = page.data as {
    deferredWorkspace?: Promise<DeferredResult<object>>;
    deferredPage?: Promise<DeferredResult<object>>;
  };
  const result = await resolvePageData(data);
  if (page.url.href !== url || page.data !== data) {
    throw new Error("Page refresh was superseded");
  }
  if (!result.ok) throw new Error(result.failure.message);
}
