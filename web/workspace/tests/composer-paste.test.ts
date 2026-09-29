import fixtureJson from "../../../tests/fixtures/composer-paste-policy.json" with {
  type: "json",
};

import {
  type ComposerPasteEvent,
  type ComposerPasteMeasurement,
  handleComposerImagePaste,
  handleComposerPaste,
  MAX_PLAIN_TEXT_PASTE_CHARS,
  MAX_PLAIN_TEXT_PASTE_LOGICAL_LINES,
  measureComposerPaste,
  namePastedImage,
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

function imagePasteEvent(clipboardData: ComposerPasteEvent["clipboardData"]) {
  let prevented = 0;
  return {
    event: {
      clipboardData,
      preventDefault: () => {
        prevented += 1;
      },
    },
    prevented: () => prevented,
  };
}

Deno.test("image paste attaches original files once without reading alternate text", () => {
  const image = new File(["png"], "image.png", { type: "image/png" });
  const { event, prevented } = imagePasteEvent({
    getData: () => {
      throw new Error("image must take priority over text");
    },
    files: [image],
    items: [{ kind: "file", type: image.type, getAsFile: () => image }],
  });
  const batches: File[][] = [];
  assertEquals(
    handleComposerImagePaste(event, (files) => batches.push(files)),
    true,
  );
  assertEquals(prevented(), 1);
  assertEquals(batches.length, 1);
  assertEquals(batches[0].length, 1);
  assertEquals(batches[0][0] === image, true);
});

Deno.test("image paste falls back to file items and ignores text, other files and null entries", () => {
  const image = new File(["jpeg"], "photo.jpg", { type: "image/jpeg" });
  const { event, prevented } = imagePasteEvent({
    getData: () => "alternate image title",
    files: [],
    items: [
      { kind: "string", type: "text/plain", getAsFile: () => null },
      {
        kind: "file",
        type: "application/pdf",
        getAsFile: () => new File([], "a.pdf"),
      },
      { kind: "file", type: "image/png", getAsFile: () => null },
      { kind: "file", type: image.type, getAsFile: () => image },
    ],
  });
  let files: File[] = [];
  assertEquals(
    handleComposerImagePaste(event, (value) => {
      files = value;
    }),
    true,
  );
  assertEquals(files.length, 1);
  assertEquals(files[0] === image, true);
  assertEquals(prevented(), 1);
});

Deno.test("multiple pasted images retain order and use attachment validation for unsupported types", () => {
  const images = ["image/png", "image/webp", "image/bmp"].map((type) =>
    new File([type], "image", { type })
  );
  const { event } = imagePasteEvent({
    getData: () => "",
    files: [
      images[0],
      new File([], "note.txt", { type: "text/plain" }),
      ...images.slice(1),
    ],
  });
  let attached: File[] = [];
  assertEquals(
    handleComposerImagePaste(event, (files) => {
      attached = files;
    }),
    true,
  );
  assertEquals(attached.length, images.length);
  assertEquals(attached.every((file, index) => file === images[index]), true);
});

Deno.test("non-image or absent clipboard leaves text paste and its exact content untouched", () => {
  for (
    const clipboardData of [null, { getData: () => "  text\r\n" }, {
      getData: () => "file name",
      files: [new File([], "file.pdf", { type: "application/pdf" })],
    }]
  ) {
    const { event, prevented } = imagePasteEvent(clipboardData);
    assertEquals(
      handleComposerImagePaste(event, () => {
        throw new Error("unexpected attachment");
      }),
      false,
    );
    assertEquals(prevented(), 0);
  }
});

Deno.test("image paste does not attach when disabled or no callback is installed", () => {
  const { event, prevented } = imagePasteEvent({
    getData: () => "",
    files: [new File(["png"], "image.png", { type: "image/png" })],
  });
  assertEquals(handleComposerImagePaste(event, undefined), false);
  assertEquals(
    handleComposerImagePaste(event, () => {
      throw new Error("disabled attachment");
    }, false),
    false,
  );
  assertEquals(prevented(), 0);
});

Deno.test("pasted screenshot names are unique without changing bytes or media type", async () => {
  const file = new File([new Uint8Array([137, 80, 78, 71])], "image.png", {
    type: "image/png",
    lastModified: 1234,
  });
  const first = namePastedImage(file);
  const second = namePastedImage(file);
  assertEquals(first.name === second.name, false);
  assertEquals(first.name.endsWith("-image.png"), true);
  assertEquals(first.type, file.type);
  assertEquals(first.lastModified, file.lastModified);
  assertEquals(Array.from(new Uint8Array(await first.arrayBuffer())), [
    137,
    80,
    78,
    71,
  ]);
});

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
