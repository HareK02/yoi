import { shouldSubmitChatKey } from "./chat-submit.ts";

declare const Deno: {
  test(name: string, fn: () => void): void;
};

function assert(condition: unknown, message: string): asserts condition {
  if (!condition) {
    throw new Error(message);
  }
}

function assertEquals<T>(actual: T, expected: T): void {
  const actualJson = JSON.stringify(actual);
  const expectedJson = JSON.stringify(expected);
  if (actualJson !== expectedJson) {
    throw new Error(`Expected ${expectedJson}, got ${actualJson}`);
  }
}

Deno.test("shouldSubmitChatKey supports platform-auto submit modifier", () => {
  assert(
    shouldSubmitChatKey(
      { key: "Enter", ctrlKey: true },
      { mode: "mod-enter", modKey: "auto", enabled: true },
    ),
    "Ctrl+Enter should submit on non-Apple platforms",
  );
  assertEquals(
    shouldSubmitChatKey(
      { key: "Enter" },
      { mode: "mod-enter", modKey: "auto", enabled: true },
    ),
    false,
  );
});

Deno.test("shouldSubmitChatKey still supports explicit Cmd+Enter behavior", () => {
  assert(
    shouldSubmitChatKey(
      { key: "Enter", metaKey: true },
      { mode: "mod-enter", modKey: "meta", enabled: true },
    ),
    "Cmd+Enter should submit when meta is selected",
  );
  assertEquals(
    shouldSubmitChatKey(
      { key: "Enter", ctrlKey: true },
      { mode: "mod-enter", modKey: "meta", enabled: true },
    ),
    false,
  );
});

Deno.test("shouldSubmitChatKey ignores IME composition and repeated Enter", () => {
  const options = {
    mode: "mod-enter" as const,
    modKey: "meta" as const,
    enabled: true,
  };
  assertEquals(
    shouldSubmitChatKey(
      { key: "Enter", metaKey: true, isComposing: true },
      options,
    ),
    false,
  );
  assertEquals(
    shouldSubmitChatKey({ key: "Enter", metaKey: true, keyCode: 229 }, options),
    false,
  );
  assertEquals(
    shouldSubmitChatKey({ key: "Enter", metaKey: true, repeat: true }, options),
    false,
  );
});

Deno.test("shouldSubmitChatKey supports enter submit mode", () => {
  const options = {
    mode: "enter" as const,
    modKey: "meta" as const,
    enabled: true,
  };
  assert(shouldSubmitChatKey({ key: "Enter" }, options), "Enter should submit");
  assertEquals(
    shouldSubmitChatKey({ key: "Enter", shiftKey: true }, options),
    false,
  );
});
