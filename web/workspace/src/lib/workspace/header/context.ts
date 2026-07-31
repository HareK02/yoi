import { getContext, setContext } from "svelte";
import type { Snippet } from "svelte";

const HEADER_CONTEXT_KEY = Symbol("workspace-header");

export type HeaderContent = Snippet<[]> | null;
export type HeaderController = {
  content: HeaderContent;
};

export function provideHeaderController(controller: HeaderController): void {
  setContext(HEADER_CONTEXT_KEY, controller);
}

export function getHeaderController(): HeaderController {
  return getContext<HeaderController>(HEADER_CONTEXT_KEY);
}
