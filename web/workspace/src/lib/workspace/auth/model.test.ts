import {
  authenticationCredentialToJson,
  base64UrlToBuffer,
  bufferToBase64Url,
  parseAuthUserResponse,
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
        authenticatorSelection: {
          residentKey: "required",
          requireResidentKey: true,
          userVerification: "required",
        },
        excludeCredentials: [{
          type: "public-key",
          id: "BwgJ" as unknown as BufferSource,
        }],
      },
    },
  });

  assertEquals(bytes(options.challenge), [1, 2, 3]);
  assertEquals(options.authenticatorSelection, {
    residentKey: "required",
    requireResidentKey: true,
    userVerification: "required",
  });
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
  assertEquals(
    parseWhoamiResponse({
      actor: null,
      authentication_reference: null,
      session: { kind: "anonymous" },
    }),
    {
      actor: null,
      authentication_reference: null,
      session: { kind: "anonymous" },
    },
  );
  assertThrows(
    () =>
      parseWhoamiResponse({
        authentication_reference: "fixture-authentication",
        session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
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
    () =>
      parseWhoamiResponse({
        actor: null,
        authentication_reference: null,
        token: "secret",
      }),
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
    parseWhoamiResponse({
      actor: null,
      authentication_reference: null,
      [attackerKey]: true,
    });
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

Deno.test("whoami fails closed on actor-state mismatch and malformed expiry", () => {
  const actor = {
    user_id: "u",
    account_id: "a",
    handle: "alice",
    display_name: "Alice",
    auth_method: "browser_session",
  };
  for (
    const payload of [
      { actor, authentication_reference: null, session: { kind: "anonymous" } },
      { actor, authentication_reference: null, session: { kind: "expired" } },
      {
        actor: null,
        authentication_reference: null,
        session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
      },
      {
        actor,
        authentication_reference: "fixture-authentication",
        session: { kind: "authenticated", expires_at: null },
      },
      {
        actor,
        authentication_reference: "fixture-authentication",
        session: { kind: "authenticated", expires_at: "tomorrow" },
      },
      {
        actor: null,
        authentication_reference: null,
        session: { kind: "future" },
      },
      {
        actor: null,
        authentication_reference: null,
        session: { kind: "expired", expires_at: "2099-01-01T00:00:00Z" },
      },
    ]
  ) assertThrows(() => parseWhoamiResponse(payload), "inconsistent whoami");
  assertEquals(
    parseWhoamiResponse({
      actor: null,
      authentication_reference: null,
      session: { kind: "expired" },
    }),
    {
      actor: null,
      authentication_reference: null,
      session: { kind: "expired" },
    },
  );
});

Deno.test("whoami and completion require explicit consistent authentication references", () => {
  const user = {
    user_id: "u",
    account_id: "a",
    handle: "alice",
    display_name: "Alice",
  };
  const authenticated = {
    actor: { ...user, auth_method: "browser_session" },
    session: { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
  } as const;
  for (const reference of [undefined, null, "", 42, "x".repeat(16385)]) {
    assertThrows(
      () =>
        parseWhoamiResponse({
          ...authenticated,
          authentication_reference: reference,
        }),
      "invalid authenticated reference",
    );
    assertThrows(
      () =>
        parseAuthUserResponse({ user, authentication_reference: reference }),
      "invalid issuance reference",
    );
  }
  for (const kind of ["anonymous", "expired"]) {
    assertThrows(
      () => parseWhoamiResponse({ actor: null, session: { kind } }),
      "missing anonymous reference",
    );
    assertThrows(
      () =>
        parseWhoamiResponse({
          actor: null,
          authentication_reference: "issued",
          session: { kind },
        }),
      "anonymous reference must be null",
    );
  }
  const completion = { user, authentication_reference: "issued" };
  assertEquals(parseAuthUserResponse(completion), completion);
  assertEquals(
    parseWhoamiResponse({
      ...authenticated,
      authentication_reference: "issued",
    }),
    {
      actor: authenticated.actor,
      authentication_reference: "issued",
      session: authenticated.session,
    },
  );
});

Deno.test("bounded future transports do not discard a created credential and required crypto bytes stay validated", () => {
  const bytes = new Uint8Array([1, 2, 3]).buffer;
  const credential = {
    id: "AQID",
    rawId: bytes,
    type: "public-key",
    getClientExtensionResults: () => ({
      credProps: { rk: true },
      prf: { results: { first: bytes } },
      largeBlob: { blob: bytes },
    }),
    response: {
      clientDataJSON: bytes,
      attestationObject: bytes,
      getTransports: () => ["internal", "hybrid", "future-bounded-transport"],
    },
  };
  const converted = registrationCredentialToJson(credential) as {
    response: { transports: string[] };
    clientExtensionResults: unknown;
  };
  assertEquals(converted.response.transports, ["internal", "hybrid"]);
  assertEquals(converted.clientExtensionResults, { credProps: { rk: true } });
  assertThrows(
    () =>
      registrationCredentialToJson({
        ...credential,
        response: { ...credential.response, clientDataJSON: "not-binary" },
      }),
    "required client data remains binary",
  );
  assertThrows(
    () =>
      registrationCredentialToJson({
        ...credential,
        response: {
          ...credential.response,
          getTransports: () => ["x".repeat(16385)],
        },
      }),
    "transport bound remains enforced",
  );
  assertThrows(
    () =>
      registrationCredentialToJson({
        ...credential,
        getClientExtensionResults: () => ({ credProps: { rk: bytes } }),
      }),
    "supported JSON extension never silently converts binary to an empty object",
  );
});
