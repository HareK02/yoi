// 本番login adapterとWebAuthn変換を接続し、HTTP/credential境界だけを代替する。
// 署名検証・credentialのdiscoverability・session発行はBackend試験の責任。
declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

import { AuthOperationError } from "./operation.ts";
import { loginWithLegacyPasskey, loginWithPasskey } from "./api.ts";
import {
  authUser,
  credentialFixture,
  legacyLoginOptions,
  loginOptions,
} from "./api.test-fixtures.ts";

function equal(actual: unknown, expected: unknown) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `Expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

async function boundary(
  run: (state: {
    requests: { path: string; init: RequestInit }[];
    credential: (
      options: CredentialRequestOptions,
    ) => Promise<Credential | null>;
  }) => Promise<void>,
) {
  const keys = ["fetch", "window", "navigator"];
  const descriptors = keys.map((key) =>
    Object.getOwnPropertyDescriptor(globalThis, key)
  );
  const state = {
    requests: [] as { path: string; init: RequestInit }[],
    credential: async (
      options: CredentialRequestOptions,
    ): Promise<Credential | null> => {
      equal(options.publicKey?.userVerification, "required");
      return credentialFixture(true);
    },
  };
  Object.defineProperties(globalThis, {
    window: {
      configurable: true,
      value: { location: { origin: "https://yoi.example" } },
    },
    navigator: {
      configurable: true,
      value: {
        credentials: {
          get: (options: CredentialRequestOptions) => state.credential(options),
        },
      },
    },
    fetch: {
      configurable: true,
      value: (input: string, init: RequestInit) => {
        state.requests.push({ path: String(input), init });
        if (String(input).endsWith("/options")) {
          const body = JSON.parse(String(init.body));
          return Promise.resolve(
            Response.json(body.handle ? legacyLoginOptions : loginOptions),
          );
        }
        return Promise.resolve(
          Response.json({
            user: authUser,
            authentication_reference: "fixture-authentication",
          }),
        );
      },
    },
  });
  try {
    await run(state);
  } finally {
    keys.forEach((key, i) => {
      const descriptor = descriptors[i];
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    });
  }
}

for (const legacy of [false, true]) {
  Deno.test(`${legacy ? "explicit compatibility" : "discoverable"} login selects only its intended options and serializes assertion`, async () => {
    await boundary(async (state) => {
      const signal = new AbortController().signal;
      state.credential = async (options) => {
        equal(options.signal, signal);
        equal(options.publicKey?.userVerification, "required");
        const allowed = options.publicKey?.allowCredentials;
        if (legacy) {
          equal(allowed?.length, 1);
          equal([...new Uint8Array(allowed![0].id as ArrayBuffer)], [7, 8, 9]);
        } else equal(allowed, []);
        return credentialFixture(true);
      };
      equal(
        await (legacy
          ? loginWithLegacyPasskey(" alice ", signal)
          : loginWithPasskey(signal)),
        { user: authUser, authentication_reference: "fixture-authentication" },
      );
      equal(state.requests.map(({ path }) => path), [
        "/api/auth/passkeys/login/options",
        "/api/auth/passkeys/login/complete",
      ]);
      equal(JSON.parse(String(state.requests[0].init.body)), {
        ...(legacy ? { handle: "alice" } : {}),
        browser_origin: "https://yoi.example",
      });
      equal(state.requests.map(({ init }) => init.credentials), [
        "same-origin",
        "same-origin",
      ]);
      const complete = JSON.parse(String(state.requests[1].init.body));
      equal(complete.challenge_id, loginOptions.challenge_id);
      equal(complete.credential.response.signature, "AQID");
      equal(complete.credential.response.userHandle, null);
    });
  });
  Deno.test(`${legacy ? "compatibility" : "discoverable"} login cancellation and late aborted credential never send completion`, async () => {
    for (const late of [false, true]) {
      await boundary(async (state) => {
        const controller = new AbortController();
        state.credential = async () => {
          if (late) controller.abort();
          return late ? credentialFixture(true) : null;
        };
        try {
          await (legacy
            ? loginWithLegacyPasskey("alice", controller.signal)
            : loginWithPasskey(controller.signal));
          throw new Error("Unexpected login success");
        } catch (error) {
          if (!(error instanceof AuthOperationError)) throw error;
          equal(error.stage, "authenticator");
          equal(error.reason, late ? "stopped" : "cancelled-or-unavailable");
          equal(error.outcome, "not-sent");
        }
        equal(state.requests.length, 1);
      });
    }
  });
}

Deno.test("explicit compatibility rejects empty handles before HTTP or WebAuthn", async () => {
  await boundary(async (state) => {
    for (const handle of ["", "  ", "\n\t"]) {
      try {
        await loginWithLegacyPasskey(handle);
        throw new Error("Unexpected login success");
      } catch (error) {
        equal(
          (error as Error).message,
          "User handle is required for legacy passkey login.",
        );
      }
    }
    equal(state.requests, []);
  });
});
