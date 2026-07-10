import { markdownToHtml } from "./markdown.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

Deno.test("markdownToHtml escapes raw html and renders basic markdown", async () => {
  const html = await markdownToHtml(
    "# Title\n\nhello **world** `<x>`\n\n<script>bad()</script>",
  );
  assert(html.includes("<h1>Title</h1>"), "heading should render");
  assert(html.includes("<strong>world</strong>"), "strong should render");
  assert(
    html.includes("&lt;script&gt;bad()&lt;/script&gt;"),
    "raw html should be escaped",
  );
  assert(
    !html.includes("<script>bad()</script>"),
    "raw html must not pass through",
  );
});

Deno.test("markdownToHtml renders fenced code through shiki", async () => {
  const html = await markdownToHtml("```ts\nconst answer: number = 42;\n```");
  assert(
    html.includes("shiki"),
    "highlighted code should include shiki markup",
  );
  assert(html.includes("answer"), "code content should be present");
});
