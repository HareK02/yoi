export type AnsiSegment = {
  text: string;
  foreground?: string;
  background?: string;
  bold: boolean;
  dim: boolean;
  italic: boolean;
  underline: boolean;
  strikethrough: boolean;
  concealed: boolean;
};

type AnsiState = Omit<AnsiSegment, "text"> & { inverse: boolean };

const ANSI_PALETTE = [
  "#1e1e1e",
  "#cd3131",
  "#0dbc79",
  "#e5e510",
  "#2472c8",
  "#bc3fbc",
  "#11a8cd",
  "#e5e5e5",
  "#666666",
  "#f14c4c",
  "#23d18b",
  "#f5f543",
  "#3b8eea",
  "#d670d6",
  "#29b8db",
  "#ffffff",
] as const;

export function ansiSegments(input: string): AnsiSegment[] {
  const segments: AnsiSegment[] = [];
  let state = defaultState();
  let text = "";

  const flush = () => {
    if (!text) return;
    segments.push(segmentFromState(text, state));
    text = "";
  };

  for (let index = 0; index < input.length;) {
    const code = input.charCodeAt(index);
    if (code !== 0x1b) {
      if (code >= 0x20 || code === 0x09 || code === 0x0a || code === 0x0d) {
        text += input[index];
      }
      index += 1;
      continue;
    }

    flush();
    const next = input[index + 1];
    if (next === "[") {
      const finalIndex = findCsiFinal(input, index + 2);
      if (finalIndex < 0) break;
      if (input[finalIndex] === "m") {
        state = applySgr(state, input.slice(index + 2, finalIndex));
      }
      index = finalIndex + 1;
      continue;
    }
    if (next === "]") {
      const finalIndex = findOscFinal(input, index + 2);
      if (finalIndex < 0) break;
      index = finalIndex;
      continue;
    }

    index += next === undefined ? 1 : 2;
  }

  flush();
  return segments;
}

function defaultState(): AnsiState {
  return {
    foreground: undefined,
    background: undefined,
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    strikethrough: false,
    concealed: false,
    inverse: false,
  };
}

function segmentFromState(text: string, state: AnsiState): AnsiSegment {
  const foreground = state.inverse
    ? state.background ?? "var(--bg)"
    : state.foreground;
  const background = state.inverse
    ? state.foreground ?? "var(--text)"
    : state.background;
  return {
    text,
    foreground,
    background,
    bold: state.bold,
    dim: state.dim,
    italic: state.italic,
    underline: state.underline,
    strikethrough: state.strikethrough,
    concealed: state.concealed,
  };
}

function findCsiFinal(input: string, start: number): number {
  for (let index = start; index < input.length; index += 1) {
    const code = input.charCodeAt(index);
    if (code >= 0x40 && code <= 0x7e) return index;
  }
  return -1;
}

function findOscFinal(input: string, start: number): number {
  for (let index = start; index < input.length; index += 1) {
    if (input.charCodeAt(index) === 0x07) return index + 1;
    if (input.charCodeAt(index) === 0x1b && input[index + 1] === "\\") {
      return index + 2;
    }
  }
  return -1;
}

function applySgr(current: AnsiState, raw: string): AnsiState {
  const state = { ...current };
  const values = raw === "" ? [0] : raw.split(";").map(parseSgrValue);
  for (let index = 0; index < values.length; index += 1) {
    const code = values[index];
    if (code === null) continue;
    if (code === 0) Object.assign(state, defaultState());
    else if (code === 1) state.bold = true;
    else if (code === 2) state.dim = true;
    else if (code === 3) state.italic = true;
    else if (code === 4) state.underline = true;
    else if (code === 7) state.inverse = true;
    else if (code === 8) state.concealed = true;
    else if (code === 9) state.strikethrough = true;
    else if (code === 22) {
      state.bold = false;
      state.dim = false;
    } else if (code === 23) state.italic = false;
    else if (code === 24) state.underline = false;
    else if (code === 27) state.inverse = false;
    else if (code === 28) state.concealed = false;
    else if (code === 29) state.strikethrough = false;
    else if (code >= 30 && code <= 37) {
      state.foreground = ANSI_PALETTE[code - 30];
    } else if (code === 38 || code === 48) {
      const parsed = parseExtendedColor(values, index + 1);
      if (parsed) {
        if (code === 38) state.foreground = parsed.color;
        else state.background = parsed.color;
        index += parsed.consumed;
      }
    } else if (code === 39) state.foreground = undefined;
    else if (code >= 40 && code <= 47) {
      state.background = ANSI_PALETTE[code - 40];
    } else if (code === 49) state.background = undefined;
    else if (code >= 90 && code <= 97) {
      state.foreground = ANSI_PALETTE[code - 82];
    } else if (code >= 100 && code <= 107) {
      state.background = ANSI_PALETTE[code - 92];
    }
  }
  return state;
}

function parseSgrValue(value: string): number | null {
  if (!/^\d+$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

function parseExtendedColor(
  values: Array<number | null>,
  start: number,
): { color: string; consumed: number } | null {
  const mode = values[start];
  const first = values[start + 1];
  if (mode === 5 && isByte(first)) {
    return { color: palette256(first), consumed: 2 };
  }
  const second = values[start + 2];
  const third = values[start + 3];
  if (mode === 2 && isByte(first) && isByte(second) && isByte(third)) {
    return {
      color: `rgb(${first}, ${second}, ${third})`,
      consumed: 4,
    };
  }
  return null;
}

function isByte(value: number | null | undefined): value is number {
  return value !== null && value !== undefined && value >= 0 && value <= 255;
}

function palette256(index: number): string {
  if (index < 16) return ANSI_PALETTE[index];
  if (index < 232) {
    const offset = index - 16;
    const red = Math.floor(offset / 36);
    const green = Math.floor((offset % 36) / 6);
    const blue = offset % 6;
    return `rgb(${colorCube(red)}, ${colorCube(green)}, ${colorCube(blue)})`;
  }
  const gray = 8 + (index - 232) * 10;
  return `rgb(${gray}, ${gray}, ${gray})`;
}

function colorCube(value: number): number {
  return value === 0 ? 0 : 55 + value * 40;
}
