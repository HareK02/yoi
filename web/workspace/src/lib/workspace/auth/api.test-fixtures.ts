// HTTP caller試験は本番adapterとWebAuthn変換を接続し、fetchとbrowser credential境界だけを代替する。
// 認証・署名の検証はBackend suiteが所有し、このfixtureでは保証しない。
export const authUser = {
  user_id: "user-alice",
  account_id: "account-alice",
  handle: "alice",
  display_name: "Alice",
};
export const registrationOptions = {
  challenge_id: "create-challenge",
  public_key: {
    publicKey: {
      challenge: "AQID",
      rp: { id: "localhost", name: "Yoi" },
      user: { id: "BAUG", name: "alice", displayName: "Alice" },
      pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      authenticatorSelection: {
        residentKey: "required",
        requireResidentKey: true,
        userVerification: "required",
      },
    },
  },
};
export const loginOptions = {
  challenge_id: "login-challenge",
  public_key: {
    publicKey: {
      challenge: "AQID",
      allowCredentials: [],
      userVerification: "required",
    },
  },
};
export const legacyLoginOptions = {
  ...loginOptions,
  public_key: {
    publicKey: {
      ...loginOptions.public_key.publicKey,
      allowCredentials: [{ type: "public-key", id: "BwgJ" }],
    },
  },
};
export function credentialFixture(authentication = false): Credential {
  const bytes = new Uint8Array([1, 2, 3]).buffer;
  return {
    id: "AQID",
    rawId: bytes,
    type: "public-key",
    authenticatorAttachment: "platform",
    getClientExtensionResults: () => ({}),
    response: authentication
      ? {
        clientDataJSON: bytes,
        authenticatorData: bytes,
        signature: bytes,
        userHandle: null,
      }
      : {
        clientDataJSON: bytes,
        attestationObject: bytes,
        getTransports: () => ["internal"],
      },
  } as unknown as Credential;
}

export function whoamiPayload(actor: unknown = null) {
  return {
    actor,
    authentication_reference: actor === null ? null : "fixture-authentication",
    session: actor === null
      ? { kind: "anonymous" }
      : { kind: "authenticated", expires_at: "2099-01-01T00:00:00Z" },
  };
}
