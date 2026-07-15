export type TextareaFitOptions = {
  value?: string;
  maxRows?: number;
};

function positiveNumber(value: number | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0
    ? value
    : fallback;
}

function pixelValue(value: string): number {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

function lineHeightPx(style: CSSStyleDeclaration): number {
  const parsed = pixelValue(style.lineHeight);
  if (parsed > 0) {
    return parsed;
  }
  const fontSize = pixelValue(style.fontSize);
  return fontSize > 0 ? fontSize * 1.2 : 16.8;
}

function verticalBoxPx(style: CSSStyleDeclaration): number {
  return pixelValue(style.paddingTop) +
    pixelValue(style.paddingBottom) +
    pixelValue(style.borderTopWidth) +
    pixelValue(style.borderBottomWidth);
}

function fitTextareaHeight(node: HTMLTextAreaElement, maxRows: number) {
  const style = globalThis.getComputedStyle(node);
  const maxHeight = lineHeightPx(style) * maxRows + verticalBoxPx(style);
  node.style.height = "auto";
  const nextHeight = Math.min(node.scrollHeight, maxHeight);
  node.style.height = `${nextHeight}px`;
  node.style.overflowY = node.scrollHeight > maxHeight ? "auto" : "hidden";
}

export function fitTextarea(
  node: HTMLTextAreaElement,
  options: TextareaFitOptions = {},
) {
  let current = {
    value: options.value,
    maxRows: positiveNumber(options.maxRows, 10),
  };
  let frame: number | null = null;

  function scheduleFit() {
    if (frame !== null) {
      return;
    }
    frame = window.requestAnimationFrame(() => {
      frame = null;
      fitTextareaHeight(node, current.maxRows);
    });
  }

  function onInput() {
    scheduleFit();
  }

  node.addEventListener("input", onInput);
  scheduleFit();

  return {
    update(nextOptions: TextareaFitOptions = {}) {
      const next = {
        value: nextOptions.value,
        maxRows: positiveNumber(nextOptions.maxRows, 10),
      };
      if (next.value !== current.value || next.maxRows !== current.maxRows) {
        current = next;
        scheduleFit();
      }
    },
    destroy() {
      if (frame !== null) {
        window.cancelAnimationFrame(frame);
      }
      node.removeEventListener("input", onInput);
    },
  };
}
