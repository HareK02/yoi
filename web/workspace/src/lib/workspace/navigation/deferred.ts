import { isHttpError, isRedirect } from "@sveltejs/kit";

export type DeferredFailure = {
  message: string;
  status: number | null;
  redirect: string | null;
};
export type DeferredResult<T> =
  | { ok: true; data: T }
  | { ok: false; failure: DeferredFailure };

// Routerへは即座に戻り、拒否されたPromiseも遷移後の表示境界で扱う。
// 読込み中のキャンセルは外部副作用の取消ではなく、古い結果の表示抑止とする。
export async function settle<T>(
  value: T | PromiseLike<T>,
): Promise<DeferredResult<T>> {
  try {
    return { ok: true, data: await value };
  } catch (error) {
    return {
      ok: false,
      failure: isRedirect(error)
        ? {
          message: "Redirecting…",
          status: error.status,
          redirect: error.location,
        }
        : {
          message: isHttpError(error)
            ? error.body.message
            : error instanceof Error
            ? error.message
            : String(error),
          status: isHttpError(error) ? error.status : null,
          redirect: null,
        },
    };
  }
}

export function deferPage<
  E extends {
    params: Record<string, string | undefined>;
    url: URL;
    parent: () => Promise<unknown>;
  },
  T extends object,
>(
  loader: (event: E) => T | Promise<T>,
): (event: E) => { deferredPage: Promise<DeferredResult<T>> } {
  return (event) => {
    // 遅延処理が読むparams/URLも、loadが返る前にRouterの依存として記録する。
    const snapshot = {
      ...event,
      params: { ...event.params },
      url: new URL(event.url.href),
    };
    // SvelteKitはparent/childを並列開始する。deferしても認証前fetchを開始しない。
    return {
      deferredPage: settle((async () => {
        await snapshot.parent();
        return await loader(snapshot);
      })()),
    };
  };
}

type Merge<A, B> = Omit<A, keyof B> & B;
type WorkspaceValue<T> = T extends
  { deferredWorkspace: Promise<DeferredResult<infer W>> } ? W : {};
type PageValue<T> = T extends { deferredPage: Promise<DeferredResult<infer P>> }
  ? P
  : {};
export type ResolvedPageData<T> = Merge<
  Merge<
    WorkspaceValue<T>,
    Omit<T, "deferredWorkspace" | "deferredPage" | "deferredCatalog">
  >,
  PageValue<T>
>;

type DeferredData = {
  deferredWorkspace?: Promise<DeferredResult<object>>;
  deferredPage?: Promise<DeferredResult<object>>;
  deferredCatalog?: unknown;
};

export async function resolvePageData<T extends DeferredData>(
  data: T,
): Promise<DeferredResult<ResolvedPageData<T>>> {
  const {
    deferredWorkspace,
    deferredPage,
    deferredCatalog: _catalog,
    ...base
  } = data;
  const [workspace, content] = await Promise.all([
    deferredWorkspace ?? settle({}),
    deferredPage ?? settle({}),
  ]);
  // Workspace取得の失敗をpage成功で補わず、権限の確認前に操作を描画しない。
  if (!workspace.ok) return workspace;
  if (!content.ok) return content;
  return {
    ok: true,
    data: { ...workspace.data, ...base, ...content.data } as ResolvedPageData<
      T
    >,
  };
}
