import type { Snippet } from "svelte";

export const WORKSPACE_SIDEBAR_CONTENT_CONTEXT = Symbol(
  "workspace-sidebar-content",
);

export type WorkspaceSidebarContentController = {
  registerContent(content: Snippet): () => void;
};
