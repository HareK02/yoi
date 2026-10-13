import type { DeferredFailure, DeferredResult } from "./deferred.ts";

// 同じ対象の再取得では確定済み表示を保持し、対象切替では旧値を即座に隠す。
export function observeDeferred<T>(
  read: () => Promise<DeferredResult<T>>,
  identity: () => string,
) {
  let settled = $state.raw<{ identity: string; value: T } | null>(null);
  let failure = $state.raw<{ identity: string; value: DeferredFailure } | null>(
    null,
  );
  let loading = $state(true);
  $effect(() => {
    const key = identity();
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
      return settled?.identity === identity() ? settled.value : null;
    },
    get failure() {
      return failure?.identity === identity() ? failure.value : null;
    },
    get loading() {
      return loading;
    },
  };
}
