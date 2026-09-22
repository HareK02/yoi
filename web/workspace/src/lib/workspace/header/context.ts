import { getContext, setContext } from "svelte";
import type { Snippet } from "svelte";

const HEADER_CONTEXT_KEY = Symbol("workspace-header");

export type HeaderSnippet = Snippet<[]>;
export type HeaderController = {
  registerContent(content: HeaderSnippet): () => void;
};

export function provideHeaderController(controller: HeaderController): void {
  setContext(HEADER_CONTEXT_KEY, controller);
}

export function getHeaderController(): HeaderController {
  return getContext<HeaderController>(HEADER_CONTEXT_KEY);
}
