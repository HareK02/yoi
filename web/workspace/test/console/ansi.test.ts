import { ansiSegments } from "../../src/lib/workspace/console/ansi.ts";

type TestRegistrar = (name: string, body: () => void) => void;

const test =
  (globalThis as unknown as { Deno: { test: TestRegistrar } }).Deno.test;

function assert(
  condition: boolean,
  message = "assertion failed",
): asserts condition {
  if (!condition) throw new Error(message);
}

function assertEquals(actual: unknown, expected: unknown): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(
      `values differ:\nactual: ${actualJson}\nexpected: ${expectedJson}`,
    );
  }
}

test("ansiSegments projects standard colors and reset", () => {
  const segments = ansiSegments("plain \x1b[31mred\x1b[0m normal");

  assertEquals(
    segments.map(({ text, foreground }) => ({ text, foreground })),
    [
      { text: "plain ", foreground: undefined },
      { text: "red", foreground: "#cd3131" },
      { text: " normal", foreground: undefined },
    ],
  );
});

test("ansiSegments supports terminal styles, 256 colors, and truecolor", () => {
  const segments = ansiSegments(
    "\x1b[1;4;38;5;202mindexed\x1b[22;24;48;2;1;2;3mbackground\x1b[0m",
  );

  assertEquals(segments[0], {
    text: "indexed",
    foreground: "rgb(255, 95, 0)",
    background: undefined,
    bold: true,
    dim: false,
    italic: false,
    underline: true,
    strikethrough: false,
    concealed: false,
  });
  assertEquals(segments[1], {
    text: "background",
    foreground: "rgb(255, 95, 0)",
    background: "rgb(1, 2, 3)",
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    strikethrough: false,
    concealed: false,
  });
});

test("ansiSegments keeps output as text and strips terminal control sequences", () => {
  const input =
    "\x1b]8;;https://example.invalid\x07<script>alert(1)</script>\x1b]8;;\x07" +
    "\x1b[2Ksafe\x00\x1b[31";
  const segments = ansiSegments(input);

  assertEquals(
    segments.map((segment) => segment.text).join(""),
    "<script>alert(1)</script>safe",
  );
  assert(!segments.some((segment) => segment.text.includes("\x1b")));
});
