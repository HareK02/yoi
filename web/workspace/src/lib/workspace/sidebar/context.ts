import { getContext } from "svelte";
import type { Snippet } from "svelte";

export type SidebarSnippet = Snippet<[]>;

export type SidebarController = {
  registerSidebar(sidebar: SidebarSnippet): () => void;
};

export const SIDEBAR_CONTEXT = Symbol("yoi-sidebar-context");

export function getSidebarController(): SidebarController {
  return getContext<SidebarController>(SIDEBAR_CONTEXT);
}
