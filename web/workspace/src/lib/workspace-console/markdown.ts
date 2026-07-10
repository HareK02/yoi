import { codeToHtml } from "shiki";

const SHIKI_THEME = "kanagawa-wave";

export async function markdownToHtml(markdown: string): Promise<string> {
  if (!markdown.trim()) {
    return "";
  }
  const blocks = splitFencedCode(markdown);
  const html: string[] = [];
  let textBuffer: string[] = [];

  async function flushText(): Promise<void> {
    if (textBuffer.length === 0) {
      return;
    }
    html.push(renderMarkdownText(textBuffer.join("\n")));
    textBuffer = [];
  }

  for (const block of blocks) {
    if (block.kind === "text") {
      textBuffer.push(block.text);
      continue;
    }
    await flushText();
    html.push(await highlightCode(block.code, block.lang));
  }
  await flushText();
  return html.join("\n");
}

type MarkdownBlock =
  | { kind: "text"; text: string }
  | { kind: "code"; lang: string; code: string };

function splitFencedCode(markdown: string): MarkdownBlock[] {
  const lines = markdown.split(/\r?\n/);
  const blocks: MarkdownBlock[] = [];
  let text: string[] = [];
  let code: string[] | null = null;
  let lang = "text";

  for (const line of lines) {
    const fence = line.match(/^```\s*([^`]*)\s*$/);
    if (fence) {
      if (code) {
        blocks.push({ kind: "text", text: text.join("\n") });
        text = [];
        blocks.push({ kind: "code", lang, code: code.join("\n") });
        code = null;
        lang = "text";
      } else {
        if (text.length > 0) {
          blocks.push({ kind: "text", text: text.join("\n") });
          text = [];
        }
        lang = normalizeLanguage(fence[1]);
        code = [];
      }
      continue;
    }
    if (code) {
      code.push(line);
    } else {
      text.push(line);
    }
  }

  if (code) {
    text.push("```" + (lang === "text" ? "" : lang));
    text.push(...code);
  }
  if (text.length > 0) {
    blocks.push({ kind: "text", text: text.join("\n") });
  }

  return blocks.filter((block) =>
    block.kind === "code" || block.text.trim().length > 0
  );
}

function renderMarkdownText(markdown: string): string {
  const lines = markdown.split(/\r?\n/);
  const html: string[] = [];
  let paragraph: string[] = [];
  let list: string[] = [];
  let blockquote: string[] = [];

  function flushParagraph(): void {
    if (paragraph.length === 0) {
      return;
    }
    html.push(`<p>${paragraph.map(renderInline).join("<br>")}</p>`);
    paragraph = [];
  }

  function flushList(): void {
    if (list.length === 0) {
      return;
    }
    html.push(
      `<ul>${
        list.map((item) => `<li>${renderInline(item)}</li>`).join("")
      }</ul>`,
    );
    list = [];
  }

  function flushBlockquote(): void {
    if (blockquote.length === 0) {
      return;
    }
    html.push(
      `<blockquote>${blockquote.map(renderInline).join("<br>")}</blockquote>`,
    );
    blockquote = [];
  }

  function flushAll(): void {
    flushParagraph();
    flushList();
    flushBlockquote();
  }

  for (const rawLine of lines) {
    const line = rawLine.trimEnd();
    if (!line.trim()) {
      flushAll();
      continue;
    }

    const heading = line.match(/^(#{1,4})\s+(.+)$/);
    if (heading) {
      flushAll();
      const level = heading[1].length;
      html.push(`<h${level}>${renderInline(heading[2])}</h${level}>`);
      continue;
    }

    const listItem = line.match(/^\s*[-*]\s+(.+)$/);
    if (listItem) {
      flushParagraph();
      flushBlockquote();
      list.push(listItem[1]);
      continue;
    }

    const quote = line.match(/^>\s?(.*)$/);
    if (quote) {
      flushParagraph();
      flushList();
      blockquote.push(quote[1]);
      continue;
    }

    flushList();
    flushBlockquote();
    paragraph.push(line);
  }
  flushAll();
  return html.join("\n");
}

function renderInline(value: string): string {
  let escaped = escapeHtml(value);
  escaped = escaped.replace(/`([^`]+)`/g, "<code>$1</code>");
  escaped = escaped.replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
  escaped = escaped.replace(/\b(https?:\/\/[^\s<]+[^\s<.,;:)])/g, (url) => {
    const safe = escapeAttribute(url);
    return `<a href="${safe}" rel="noreferrer" target="_blank">${url}</a>`;
  });
  return escaped;
}

async function highlightCode(code: string, lang: string): Promise<string> {
  try {
    return await codeToHtml(code, {
      lang: lang || "text",
      theme: SHIKI_THEME,
    });
  } catch {
    return `<pre class="shiki fallback"><code>${escapeHtml(code)}</code></pre>`;
  }
}

function normalizeLanguage(value: string | undefined): string {
  const raw = value?.trim().split(/\s+/, 1)[0] ?? "";
  if (!raw) {
    return "text";
  }
  switch (raw) {
    case "sh":
    case "shell":
    case "zsh":
      return "bash";
    case "ts":
      return "typescript";
    case "js":
      return "javascript";
    default:
      return raw;
  }
}

function escapeHtml(value: string): string {
  return value.replace(/[&<>"]/g, (char) => {
    switch (char) {
      case "&":
        return "&amp;";
      case "<":
        return "&lt;";
      case ">":
        return "&gt;";
      case '"':
        return "&quot;";
      default:
        return char;
    }
  });
}

function escapeAttribute(value: string): string {
  return escapeHtml(value).replace(/'/g, "&#39;");
}
