import {
  base64UrlToBuffer,
  bufferToBase64Url,
  prepareLoginOptions,
  prepareRegistrationOptions,
} from "./model.ts";

declare const Deno: {
  test(name: string, fn: () => void | Promise<void>): void;
};

function assertEquals<T>(actual: T, expected: T): void {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

function bytes(buffer: BufferSource): number[] {
  if (buffer instanceof ArrayBuffer) {
    return [...new Uint8Array(buffer)];
  }
  return [...new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength)];
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
      challenge: "AQID" as unknown as BufferSource,
      rp: { id: "localhost", name: "Yoi" },
      user: {
        id: "BAUG" as unknown as BufferSource,
        name: "local",
        displayName: "Local User",
      },
      pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      excludeCredentials: [{ type: "public-key", id: "BwgJ" as unknown as BufferSource }],
    },
  });

  assertEquals(bytes(options.challenge), [1, 2, 3]);
  assertEquals(bytes(options.user.id), [4, 5, 6]);
  assertEquals(bytes(options.excludeCredentials?.[0].id as BufferSource), [7, 8, 9]);
});

Deno.test("prepareLoginOptions decodes challenge and allowed credential ids", () => {
  const options = prepareLoginOptions({
    challenge_id: "challenge-1",
    public_key: {
      challenge: "AQID" as unknown as BufferSource,
      allowCredentials: [{ type: "public-key", id: "BwgJ" as unknown as BufferSource }],
    },
  });

  assertEquals(bytes(options.challenge), [1, 2, 3]);
  assertEquals(bytes(options.allowCredentials?.[0].id as BufferSource), [7, 8, 9]);
});
