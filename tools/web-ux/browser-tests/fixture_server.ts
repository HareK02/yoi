const port = Number(Deno.args[0]);
if (!Number.isInteger(port) || port <= 0) throw new Error("port is required");

const canary = Deno.env.get("WEB_UX_FIXTURE_SECRET") ?? "";
console.log(`Authorization: Bearer ${canary}`);

Deno.serve({ hostname: "127.0.0.1", port }, (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/health") return new Response("ok");
  const cookie = request.headers.get("cookie") ?? "";
  const owner = cookie.includes("persona=owner");
  const title = owner ? "Owner repository settings" : "Repository settings";
  const action = owner
    ? '<button type="button">Add repository</button>'
    : '<p role="note">Ask a Workspace owner to change repository access.</p>';
  return new Response(
    `<!doctype html><html><head><title>${title}</title><style>body{font:16px system-ui;margin:0}main{max-width:800px;margin:40px auto}header{border-bottom:1px solid #ccc;padding:16px}section{border:1px solid #ccc;padding:20px}button{background:#06c;color:white;padding:10px 20px}</style></head><body><header>Workspace</header><main><h1>${title}</h1><section><h2>main</h2><p>SSH repository access is configured.</p>${action}<span data-web-ux-redact>${canary}</span><script>for(let index=0;index<150;index++)console.error('fixture error '+index)</script></section></main></body></html>`,
    { headers: { "content-type": "text/html; charset=utf-8" } },
  );
});
