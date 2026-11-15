import { tick } from "svelte";

export type TranscriptAnchor = { id: string; top: number };

export function isNearTranscriptBottom(element: HTMLElement, threshold = 48): boolean {
  return element.scrollHeight - element.scrollTop - element.clientHeight <= threshold;
}

export function captureTranscriptAnchor(
  container: HTMLElement | null,
  selector: string,
  dataKey: string,
): TranscriptAnchor | null {
  const first = container?.querySelector<HTMLElement>(selector);
  const id = first?.dataset[dataKey] ?? "";
  return first && id ? { id, top: first.getBoundingClientRect().top } : null;
}

export function restoreTranscriptAnchor(
  container: HTMLElement | null,
  selector: string,
  anchor: TranscriptAnchor | null,
): void {
  if (!anchor || !container) return;
  const target = container.querySelector<HTMLElement>(selector);
  if (target) container.scrollTop += target.getBoundingClientRect().top - anchor.top;
}

// DOM更新の間に対象切替やユーザーのスクロールがあれば、古い追従を適用しない。
export async function followTranscriptTail(
  element: HTMLElement | null,
  shouldFollow: () => boolean,
): Promise<boolean> {
  await tick();
  if (!element || !element.isConnected || !shouldFollow()) return false;
  element.scrollTop = element.scrollHeight;
  return true;
}

// Markdownの非同期描画や折返し変更も、新着と同じ追従条件に従う。
export function observeTranscriptSize(
  element: HTMLElement,
  changed: () => void,
  viewport?: HTMLElement | null,
): () => void {
  if (typeof ResizeObserver === "undefined") return () => {};
  const observer = new ResizeObserver(changed);
  observer.observe(element);
  if (viewport) observer.observe(viewport);
  return () => observer.disconnect();
}
