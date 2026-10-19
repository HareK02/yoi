import { error, redirect } from "@sveltejs/kit";
import { browserSession, observeBrowserSession } from "./session-state.ts";
import { isPublicPage, loginUrl } from "./routes.ts";

// root loadとそのparentを待つ各loaderが同じ確認を共有する。
// SSRは無効。fragmentはHTTP/Serverに依存せず、browserの最終URLから回収する。
export async function requirePageSession(url: URL): Promise<void> {
  observeBrowserSession();
  if (isPublicPage(url.pathname)) return;
  const snapshot = await browserSession.refresh();
  if (snapshot !== browserSession.snapshot) return requirePageSession(url);
  if (snapshot.kind !== "ready") {
    error(
      503,
      "Your session could not be loaded. Login status is unknown. Retry this page.",
    );
  }
  if (snapshot.value.session.kind === "authenticated") return;
  if (snapshot.value.session.kind === "anonymous") redirect(307, "/");
  const target = new URL(url);
  if (
    typeof location !== "undefined" && location.pathname === url.pathname &&
    location.search === url.search
  ) {
    target.hash = location.hash;
  }
  redirect(307, loginUrl(target.pathname + target.search + target.hash));
}
