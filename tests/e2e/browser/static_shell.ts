// 実Backendへの透過proxyと本番SPA配信をbrowser suite間で共有する。
// Cookie/Origin/署名検証/認可は置換しない。fixture入口は呼出側が明示的に渡す。
import { extname, join, normalize } from "@std/path";

export async function serveProductionShell(
  backend: string,
  origin: string,
  build: string,
  fixture?: (request: Request) => Promise<Response | undefined>,
) {
  for (const address of [backend, origin]) {
    const url = new URL(address);
    if (
      url.protocol !== "http:" ||
      !["localhost", "127.0.0.1"].includes(url.hostname) || !url.port
    ) {
      throw new Error("Browser E2E requires explicit owned loopback endpoints");
    }
  }
  await Deno.stat(join(build, "index.html")); // 暗黙buildや別asset rootへのfallbackはしない。
  const mime: Record<string, string> = {
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript",
    ".css": "text/css",
    ".json": "application/json",
    ".svg": "image/svg+xml",
    ".png": "image/png",
    ".woff2": "font/woff2",
    ".ico": "image/x-icon",
  };
  return Deno.serve({
    hostname: "127.0.0.1",
    port: Number(new URL(origin).port),
    onListen() {},
  }, async (request) => {
    const url = new URL(request.url);
    const special = await fixture?.(request);
    if (special) return special;
    if (url.pathname.startsWith("/api/")) {
      const headers = new Headers(request.headers);
      headers.delete("host");
      headers.delete("connection");
      return await fetch(new URL(url.pathname + url.search, backend), {
        method: request.method,
        headers,
        body: ["GET", "HEAD"].includes(request.method)
          ? undefined
          : await request.arrayBuffer(),
        redirect: "manual",
      });
    }
    const relative = normalize(
      url.pathname.replace(/^\/+/, "") || "index.html",
    );
    if (relative.startsWith("..")) {
      return new Response("not found", { status: 404 });
    }
    let file = join(build, relative);
    try {
      if ((await Deno.stat(file)).isDirectory) file = join(file, "index.html");
      return new Response(await Deno.readFile(file), {
        headers: {
          "content-type": mime[extname(file)] ?? "application/octet-stream",
        },
      });
    } catch (error) {
      if (!(error instanceof Deno.errors.NotFound)) throw error;
      if (url.pathname.startsWith("/_app/")) {
        return new Response("not found", { status: 404 });
      }
      return new Response(await Deno.readFile(join(build, "index.html")), {
        headers: { "content-type": mime[".html"] },
      });
    }
  });
}
