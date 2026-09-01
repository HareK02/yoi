import fixtureJson from "../../../tests/fixtures/composer-paste-policy.json" with {
  type: "json",
};

import {
  type ComposerPasteMeasurement,
  handleComposerPaste,
  MAX_PLAIN_TEXT_PASTE_CHARS,
  MAX_PLAIN_TEXT_PASTE_LOGICAL_LINES,
  measureComposerPaste,
} from "../src/lib/workspace/console/composer-paste.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

function assertEquals(
  actual: unknown,
  expected: unknown,
  message?: string,
): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(
      `${
        message ? `${message}: ` : ""
      }expected ${expectedJson}, got ${actualJson}`,
    );
  }
}

interface FixturePart {
  value: string;
  repeat: number;
}

interface FixtureCase {
  name: string;
  parts: FixturePart[];
  char_count: number;
  logical_line_count: number;
  presentation: "text" | "chip";
}

interface PastePolicyFixture {
  max_plain_text_chars: number;
  max_plain_text_logical_lines: number;
  cases: FixtureCase[];
}

const fixture = fixtureJson as PastePolicyFixture;

function fixtureContent(testCase: FixtureCase): string {
  return testCase.parts.map(({ value, repeat }) => value.repeat(repeat)).join(
    "",
  );
}

Deno.test("Browser composer follows the shared paste presentation contract", () => {
  assertEquals(MAX_PLAIN_TEXT_PASTE_CHARS, fixture.max_plain_text_chars);
  assertEquals(
    MAX_PLAIN_TEXT_PASTE_LOGICAL_LINES,
    fixture.max_plain_text_logical_lines,
  );

  for (const testCase of fixture.cases) {
    assertEquals(
      measureComposerPaste(fixtureContent(testCase)),
      {
        charCount: testCase.char_count,
        logicalLineCount: testCase.logical_line_count,
        presentation: testCase.presentation,
      },
      testCase.name,
    );
  }
});

Deno.test("short paste remains a native Browser edit", () => {
  let prevented = false;
  let inserted = 0;
  const handled = handleComposerPaste(
    {
      clipboardData: { getData: () => "replace the selection" },
      preventDefault: () => {
        prevented = true;
      },
    },
    () => {
      inserted += 1;
    },
  );

  assertEquals(handled, false);
  assertEquals(prevented, false);
  assertEquals(inserted, 0);
});

Deno.test("chip paste is prevented and routed exactly once", () => {
  const content = "🦀".repeat(51);
  let prevented = 0;
  const inserted: Array<[string, ComposerPasteMeasurement]> = [];
  const handled = handleComposerPaste(
    {
      clipboardData: { getData: () => content },
      preventDefault: () => {
        prevented += 1;
      },
    },
    (paste, measurement) => inserted.push([paste, measurement]),
  );

  assertEquals(handled, true);
  assertEquals(prevented, 1);
  assertEquals(inserted, [[content, {
    charCount: 51,
    logicalLineCount: 1,
    presentation: "chip",
  }]]);
});
