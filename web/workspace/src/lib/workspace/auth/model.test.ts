import {
  authenticationCredentialToJson,
  base64UrlToBuffer,
  bufferToBase64Url,
  parseDeviceLoginPollResponse,
  parseDeviceLoginStartResponse,
  parseWhoamiResponse,
  prepareLoginOptions,
  prepareRegistrationOptions,
  registrationCredentialToJson,
} from "./model.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function assertEquals<T>(actual: T, expected: T): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
    );
  }
}

function assertThrows(fn: () => unknown, message: string): void {
  try {
    fn();
  } catch (error) {
    if (
      !(error instanceof Error) ||
      !error.message.startsWith("Invalid auth payload:")
    ) {
      throw new Error(`${message}: unexpected error`);
    }
    return;
  }
  throw new Error(`${message}: expected an error`);
}

function bytes(buffer: BufferSource): number[] {
  if (buffer instanceof ArrayBuffer) {
    return [...new Uint8Array(buffer)];
  }
  return [
    ...new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength),
  ];
}

Deno.test("base64url helpers round-trip binary data", () => {
  const source = new Uint8Array([0, 1, 2, 250, 251, 252, 253, 254, 255]).buffer;
  const encoded = bufferToBase64Url(source);
  assertEquals(encoded.includes("+"), false);
  assertEquals(encoded.includes("/"), false);
  assertEquals(encoded.includes("="), false);
  assertEquals(bytes(base64UrlToBuffer(encoded)), bytes(source));
});

Deno.test("prepareRegistrationOptions decodes binary public key fields", () => {
  const options = prepareRegistrationOptions({
    challenge_id: "challenge-1",
    public_key: {
      publicKey: {
        challenge: "AQID" as unknown as BufferSource,
        rp: { id: "localhost", name: "Yoi" },
        user: {
          id: "BAUG" as unknown as BufferSource,
          name: "local",
          displayName: "Local User",
        },
        pubKeyCredParams: [{ type: "public-key", alg: -7 }],
        excludeCredentials: [{
          type: "public-key",
          id: "BwgJ" as unknown as BufferSource,
        }],
      },
    },
  });

  assertEquals(bytes(options.challenge), [1, 2, 3]);
  assertEquals(bytes(options.user.id), [4, 5, 6]);
  assertEquals(bytes(options.excludeCredentials?.[0].id as BufferSource), [
    7,
    8,
    9,
  ]);
});

Deno.test("prepareLoginOptions decodes challenge and allowed credential ids", () => {
  const options = prepareLoginOptions({
    challenge_id: "challenge-1",
    public_key: {
      publicKey: {
        challenge: "AQID" as unknown as BufferSource,
        allowCredentials: [{
          type: "public-key",
          id: "BwgJ" as unknown as BufferSource,
        }],
      },
    },
  });

  assertEquals(bytes(options.challenge), [1, 2, 3]);
  assertEquals(bytes(options.allowCredentials?.[0].id as BufferSource), [
    7,
    8,
    9,
  ]);
});

Deno.test("whoami rejects unknown auth methods and extra fields", () => {
  assertEquals(parseWhoamiResponse({ actor: null }), { actor: null });
  assertThrows(
    () =>
      parseWhoamiResponse({
        actor: {
          user_id: "user-1",
          account_id: "account-1",
          handle: "hare",
          display_name: "Hare",
          auth_method: "future_method",
        },
      }),
    "unknown auth method",
  );
  assertThrows(
    () => parseWhoamiResponse({ actor: null, token: "secret" }),
    "unexpected whoami field",
  );
});

Deno.test("device login rejects unsafe expiry and unknown status", () => {
  const start = {
    device_code: "device-1",
    user_code: "ABCD-EFGH",
    verification_uri: "https://yoi.example/login/device",
    verification_uri_complete:
      "https://yoi.example/login/device?user_code=ABCD-EFGH",
    expires_in: 600,
    interval: 2,
  };
  assertEquals(parseDeviceLoginStartResponse(start), start);
  assertThrows(
    () =>
      parseDeviceLoginStartResponse({
        ...start,
        expires_in: Number.MAX_SAFE_INTEGER + 1,
      }),
    "unsafe expiry",
  );
  assertEquals(parseDeviceLoginPollResponse({ status: "denied" }), {
    status: "denied",
  });
  assertThrows(
    () => parseDeviceLoginPollResponse({ status: "future_status" }),
    "unknown device-login status",
  );
  assertThrows(
    () => parseDeviceLoginPollResponse({ status: "approved" }),
    "approved response without token",
  );
});

Deno.test("auth validation enforces cumulative budgets without echoing unknown keys", () => {
  const extensionArrays = Object.fromEntries(
    Array.from({ length: 9 }, (_, index) => [
      `field-${index}`,
      Array.from({ length: 128 }, () => 1),
    ]),
  );
  assertThrows(
    () =>
      prepareLoginOptions({
        challenge_id: "challenge-1",
        public_key: {
          publicKey: {
            challenge: "AQID",
            extensions: extensionArrays,
          },
        },
      }),
    "cumulative value budget",
  );

  const attackerKey = `secret-${"x".repeat(512)}`;
  try {
    parseWhoamiResponse({ actor: null, [attackerKey]: true });
    throw new Error("expected an error");
  } catch (error) {
    if (!(error instanceof Error) || error.message.includes(attackerKey)) {
      throw new Error("diagnostic included an attacker-controlled key");
    }
  }
});

Deno.test("passkey credential conversion fails closed on malformed payloads", () => {
  const bytes = new Uint8Array([1, 2, 3]).buffer;
  const registration = {
    id: "AQID",
    rawId: bytes,
    type: "public-key",
    authenticatorAttachment: "platform",
    getClientExtensionResults: () => ({ credProps: { rk: true } }),
    response: {
      clientDataJSON: bytes,
      attestationObject: bytes,
      getTransports: () => ["internal"],
    },
  };
  const converted = registrationCredentialToJson(registration) as Record<
    string,
    unknown
  >;
  assertEquals(converted.id, "AQID");
  assertEquals(converted.clientExtensionResults, { credProps: { rk: true } });

  assertThrows(
    () => registrationCredentialToJson({ ...registration, rawId: "AQID" }),
    "registration rawId string",
  );
  assertThrows(
    () =>
      authenticationCredentialToJson({ ...registration, type: "future-key" }),
    "unknown credential type",
  );
});
