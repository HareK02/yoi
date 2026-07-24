export type AuthenticatedUser = {
  user_id: string;
  account_id: string;
  handle: string;
  display_name: string;
};

export type RequestActor = AuthenticatedUser & {
  auth_method: "browser_session" | "api_token" | string;
};

export type WhoamiResponse = {
  actor: RequestActor | null;
};

export type PasskeyRegistrationOptionsResponse = {
  challenge_id: string;
  public_key: PublicKeyCredentialCreationOptions | {
    publicKey: PublicKeyCredentialCreationOptions;
  };
};

export type PasskeyLoginOptionsResponse = {
  challenge_id: string;
  public_key: PublicKeyCredentialRequestOptions | {
    publicKey: PublicKeyCredentialRequestOptions;
  };
};

export type PasskeyUserResponse = {
  user: AuthenticatedUser;
};

export type DeviceApprovalResponse = {
  status: string;
};

export type RegistrationCredentialJson = {
  id: string;
  rawId: string;
  type: string;
  response: {
    clientDataJSON: string;
    attestationObject: string;
    transports: string[];
  };
};

export type AuthenticationCredentialJson = {
  id: string;
  rawId: string;
  type: string;
  response: {
    clientDataJSON: string;
    authenticatorData: string;
    signature: string;
    userHandle: string | null;
  };
};

export function base64UrlToBuffer(value: string): ArrayBuffer {
  const padding = "=".repeat((4 - (value.length % 4)) % 4);
  const base64 = `${value}${padding}`.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(base64);
  return Uint8Array.from(binary, (char) => char.charCodeAt(0)).buffer;
}

export function bufferToBase64Url(buffer: ArrayBuffer): string {
  const bytes = new Uint8Array(buffer);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(
    /=+$/g,
    "",
  );
}

function unwrapPublicKey<T>(value: T | { publicKey: T }): T {
  if (value && typeof value === "object" && "publicKey" in value) {
    return (value as { publicKey: T }).publicKey;
  }
  return value as T;
}

export function prepareRegistrationOptions(
  options: PasskeyRegistrationOptionsResponse,
): PublicKeyCredentialCreationOptions {
  const publicKey = structuredClone(unwrapPublicKey(options.public_key));
  publicKey.challenge = base64UrlToBuffer(
    publicKey.challenge as unknown as string,
  );
  publicKey.user = {
    ...publicKey.user,
    id: base64UrlToBuffer(publicKey.user.id as unknown as string),
  };
  publicKey.excludeCredentials = publicKey.excludeCredentials?.map((
    credential,
  ) => ({
    ...credential,
    id: base64UrlToBuffer(credential.id as unknown as string),
  }));
  return publicKey;
}

export function prepareLoginOptions(
  options: PasskeyLoginOptionsResponse,
): PublicKeyCredentialRequestOptions {
  const publicKey = structuredClone(unwrapPublicKey(options.public_key));
  publicKey.challenge = base64UrlToBuffer(
    publicKey.challenge as unknown as string,
  );
  publicKey.allowCredentials = publicKey.allowCredentials?.map((
    credential,
  ) => ({
    ...credential,
    id: base64UrlToBuffer(credential.id as unknown as string),
  }));
  return publicKey;
}

export function registrationCredentialToJson(
  credential: PublicKeyCredential,
): RegistrationCredentialJson {
  const response = credential.response as AuthenticatorAttestationResponse;
  return {
    id: credential.id,
    rawId: bufferToBase64Url(credential.rawId),
    type: credential.type,
    response: {
      clientDataJSON: bufferToBase64Url(response.clientDataJSON),
      attestationObject: bufferToBase64Url(response.attestationObject),
      transports: response.getTransports?.() ?? [],
    },
  };
}

export function authenticationCredentialToJson(
  credential: PublicKeyCredential,
): AuthenticationCredentialJson {
  const response = credential.response as AuthenticatorAssertionResponse;
  return {
    id: credential.id,
    rawId: bufferToBase64Url(credential.rawId),
    type: credential.type,
    response: {
      clientDataJSON: bufferToBase64Url(response.clientDataJSON),
      authenticatorData: bufferToBase64Url(response.authenticatorData),
      signature: bufferToBase64Url(response.signature),
      userHandle: response.userHandle
        ? bufferToBase64Url(response.userHandle)
        : null,
    },
  };
}

export function isPublicKeyCredential(
  credential: Credential | null,
): credential is PublicKeyCredential {
  return credential != null && credential.type === "public-key";
}
