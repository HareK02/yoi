import type { DeferredFailure, DeferredResult } from "./deferred.ts";

// 同じ対象の再取得では確定済み表示を保持し、対象切替では旧値を即座に隠す。
export function observeDeferred<T>(
  read: () => Promise<DeferredResult<T>>,
  identity: () => string | undefined,
) {
  let settled = $state.raw<{ identity: string; value: T } | null>(null);
  let failure = $state.raw<{ identity: string; value: DeferredFailure } | null>(
    null,
  );
  let loading = $state(true);
  $effect(() => {
    const key = identity();
    // Leaving a scoped route can remove its params before the layout is destroyed.
    if (key === undefined) {
      loading = false;
      failure = null;
      return;
    }
    const promise = read();
    let active = true;
    loading = true;
    failure = null;
    void promise.then((result) => {
      if (!active) return;
      loading = false;
      if (result.ok) settled = { identity: key, value: result.data };
      else {
        settled = null;
        failure = { identity: key, value: result.failure };
      }
    });
    return () => {
      active = false;
    };
  });
  return {
    get data() {
      const key = identity();
      return settled !== null && settled.identity === key ? settled.value : null;
    },
    get failure() {
      const key = identity();
      return failure !== null && failure.identity === key ? failure.value : null;
    },
    get loading() {
      return loading;
    },
  };
}
