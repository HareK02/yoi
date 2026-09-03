import type {
  ActorAuthMethod,
  AuthenticatedUser,
  AuthPublicConfig,
  AuthUserResponse,
  DeviceLoginApproveResponse,
  DeviceLoginPollResponse,
  DeviceLoginPollStatus,
  DeviceLoginStartResponse,
  LogoutResponse,
  PasskeyLoginOptionsResponse,
  PasskeyRegistrationOptionsResponse,
  RequestActor,
  WhoamiResponse,
} from "$lib/generated/auth-api.ts";

export type AuthUser = AuthenticatedUser;
export type PasskeyUserResponse = AuthUserResponse;
export type DeviceApprovalResponse = DeviceLoginApproveResponse;
export type {
  AuthPublicConfig,
  DeviceLoginPollResponse,
  DeviceLoginStartResponse,
  PasskeyLoginOptionsResponse,
  PasskeyRegistrationOptionsResponse,
  RequestActor,
  WhoamiResponse,
};

const MAX_AUTH_STRING_LENGTH = 16 * 1024;
const MAX_AUTH_ARRAY_LENGTH = 128;
const MAX_AUTH_OBJECT_KEYS = 128;
const MAX_AUTH_VALUE_DEPTH = 8;
const MAX_AUTH_VALUE_NODES = 1_024;
const MAX_AUTH_VALUE_STRING_UNITS = 64 * 1024;
const MAX_DEVICE_LOGIN_SECONDS = 24 * 60 * 60;
const MAX_WEBAUTHN_TIMEOUT_MS = 10 * 60 * 1000;
const BASE64URL_PATTERN = /^[A-Za-z0-9_-]+={0,2}$/;

function invalid(path: string, reason: string): never {
  throw new Error(`Invalid auth payload: ${path} ${reason}.`);
}

function asRecord(value: unknown, path: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    invalid(path, "must be an object");
  }
  return value as Record<string, unknown>;
}

function requireExactKeys(
  value: Record<string, unknown>,
  path: string,
  required: readonly string[],
  optional: readonly string[] = [],
): void {
  const allowed = new Set([...required, ...optional]);
  for (const key of required) {
    if (!(key in value)) invalid(`${path}.${key}`, "is required");
  }
  for (const key of Object.keys(value)) {
    if (key.length === 0 || key.length > 256) {
      invalid(path, "has an invalid field name");
    }
    if (!allowed.has(key)) invalid(path, "contains an unknown field");
  }
}

function boundedString(
  value: unknown,
  path: string,
  { allowEmpty = false }: { allowEmpty?: boolean } = {},
): string {
  if (typeof value !== "string") invalid(path, "must be a string");
  if (
    (!allowEmpty && value.length === 0) || value.length > MAX_AUTH_STRING_LENGTH
  ) {
    invalid(path, "has an invalid length");
  }
  return value;
}

function optionalString(value: unknown, path: string): string | null {
  if (value === null || value === undefined) return null;
  return boundedString(value, path);
}

function positiveSafeInteger(
  value: unknown,
  path: string,
  maximum: number,
): number {
  if (
    typeof value !== "number" ||
    !Number.isSafeInteger(value) ||
    value <= 0 ||
    value > maximum
  ) {
    invalid(path, "must be a bounded positive safe integer");
  }
  return value;
}

interface ValidationBudget {
  remainingNodes: number;
  remainingStringUnits: number;
}

function boundedJson(
  value: unknown,
  path: string,
  depth = 0,
  budget: ValidationBudget = {
    remainingNodes: MAX_AUTH_VALUE_NODES,
    remainingStringUnits: MAX_AUTH_VALUE_STRING_UNITS,
  },
): void {
  budget.remainingNodes -= 1;
  if (budget.remainingNodes < 0) invalid(path, "has too many values");
  if (depth > MAX_AUTH_VALUE_DEPTH) invalid(path, "is too deeply nested");
  if (value === null || typeof value === "boolean") return;
  if (typeof value === "string") {
    boundedString(value, path, { allowEmpty: true });
    budget.remainingStringUnits -= value.length;
    if (budget.remainingStringUnits < 0) {
      invalid(path, "has too much string data");
    }
    return;
  }
  if (typeof value === "number") {
    if (!Number.isFinite(value)) invalid(path, "must be finite");
    return;
  }
  if (Array.isArray(value)) {
    if (value.length > MAX_AUTH_ARRAY_LENGTH) {
      invalid(path, "has too many items");
    }
    value.forEach((item, index) =>
      boundedJson(item, `${path}[${index}]`, depth + 1, budget)
    );
    return;
  }
  const record = asRecord(value, path);
  const keys = Object.keys(record);
  if (keys.length > MAX_AUTH_OBJECT_KEYS) invalid(path, "has too many fields");
  for (const key of keys) {
    if (key.length === 0 || key.length > 256) {
      invalid(path, "has an invalid field name");
    }
    budget.remainingStringUnits -= key.length;
    if (budget.remainingStringUnits < 0) {
      invalid(path, "has too much field-name data");
    }
    boundedJson(record[key], `${path}.field`, depth + 1, budget);
  }
}

function base64Url(value: unknown, path: string): string {
  const encoded = boundedString(value, path);
  if (!BASE64URL_PATTERN.test(encoded)) invalid(path, "must be base64url");
  return encoded;
}

function fromBase64Url(value: unknown, path: string): ArrayBuffer {
  const encoded = base64Url(value, path);
  try {
    const normalized = encoded.replace(/-/g, "+").replace(/_/g, "/");
    const padded = normalized.padEnd(Math.ceil(normalized.length / 4) * 4, "=");
    const binary = atob(padded);
    const bytes = Uint8Array.from(
      binary,
      (character) => character.charCodeAt(0),
    );
    if (bytes.byteLength === 0 || bytes.byteLength > MAX_AUTH_STRING_LENGTH) {
      invalid(path, "decodes to an invalid length");
    }
    return bytes.buffer;
  } catch {
    invalid(path, "must be valid base64url");
  }
}

function toBase64Url(value: unknown, path: string): string {
  if (!(value instanceof ArrayBuffer)) invalid(path, "must be an ArrayBuffer");
  if (value.byteLength === 0 || value.byteLength > MAX_AUTH_STRING_LENGTH) {
    invalid(path, "has an invalid byte length");
  }
  const bytes = new Uint8Array(value);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(
    /=+$/g,
    "",
  );
}

export function base64UrlToBuffer(value: string): ArrayBuffer {
  return fromBase64Url(value, "base64url");
}

export function bufferToBase64Url(value: ArrayBuffer): string {
  return toBase64Url(value, "buffer");
}

function parseAuthenticatorTransport(
  value: unknown,
  path: string,
): AuthenticatorTransport {
  const transport = boundedString(value, path);
  if (!["ble", "hybrid", "internal", "nfc", "usb"].includes(transport)) {
    invalid(path, "contains an unknown authenticator transport");
  }
  return transport as AuthenticatorTransport;
}

function parseCredentialDescriptor(
  value: unknown,
  path: string,
): PublicKeyCredentialDescriptor {
  const descriptor = asRecord(value, path);
  requireExactKeys(descriptor, path, ["type", "id"], ["transports"]);
  if (descriptor.type !== "public-key") {
    invalid(`${path}.type`, "must be public-key");
  }
  const transports = descriptor.transports === undefined ? undefined : (() => {
    if (!Array.isArray(descriptor.transports)) {
      invalid(`${path}.transports`, "must be an array");
    }
    if (descriptor.transports.length > MAX_AUTH_ARRAY_LENGTH) {
      invalid(`${path}.transports`, "has too many items");
    }
    return descriptor.transports.map((transport, index) =>
      parseAuthenticatorTransport(transport, `${path}.transports[${index}]`)
    );
  })();
  return {
    type: "public-key",
    id: fromBase64Url(descriptor.id, `${path}.id`),
    ...(transports === undefined ? {} : { transports }),
  };
}

function unwrapPublicKey(
  value: unknown,
  path: string,
): Record<string, unknown> {
  const envelope = asRecord(value, path);
  if ("publicKey" in envelope) {
    requireExactKeys(envelope, path, ["publicKey"]);
    const publicKey = asRecord(envelope.publicKey, `${path}.publicKey`);
    boundedJson(publicKey, `${path}.publicKey`);
    return publicKey;
  }
  boundedJson(envelope, path);
  return envelope;
}

function parseCreationOptions(
  value: unknown,
): PublicKeyCredentialCreationOptions {
  const options = unwrapPublicKey(value, "public_key");
  for (
    const field of ["rp", "user", "challenge", "pubKeyCredParams"] as const
  ) {
    if (!(field in options)) {
      invalid(
        `public_key.publicKey.${field}`,
        "is required",
      );
    }
  }

  const rp = asRecord(options.rp, "public_key.publicKey.rp");
  requireExactKeys(rp, "public_key.publicKey.rp", ["name"], ["id"]);
  const normalizedRp: PublicKeyCredentialRpEntity = {
    name: boundedString(rp.name, "public_key.publicKey.rp.name"),
    ...(rp.id === undefined
      ? {}
      : { id: boundedString(rp.id, "public_key.publicKey.rp.id") }),
  };

  const user = asRecord(options.user, "public_key.publicKey.user");
  requireExactKeys(user, "public_key.publicKey.user", [
    "id",
    "name",
    "displayName",
  ]);
  const normalizedUser: PublicKeyCredentialUserEntity = {
    id: fromBase64Url(user.id, "public_key.publicKey.user.id"),
    name: boundedString(user.name, "public_key.publicKey.user.name"),
    displayName: boundedString(
      user.displayName,
      "public_key.publicKey.user.displayName",
    ),
  };

  if (
    !Array.isArray(options.pubKeyCredParams) ||
    options.pubKeyCredParams.length === 0
  ) {
    invalid(
      "public_key.publicKey.pubKeyCredParams",
      "must be a non-empty array",
    );
  }
  if (options.pubKeyCredParams.length > MAX_AUTH_ARRAY_LENGTH) {
    invalid("public_key.publicKey.pubKeyCredParams", "has too many items");
  }
  const pubKeyCredParams = options.pubKeyCredParams.map((value, index) => {
    const parameter = asRecord(
      value,
      `public_key.publicKey.pubKeyCredParams[${index}]`,
    );
    requireExactKeys(
      parameter,
      `public_key.publicKey.pubKeyCredParams[${index}]`,
      ["type", "alg"],
    );
    if (parameter.type !== "public-key") {
      invalid(
        `public_key.publicKey.pubKeyCredParams[${index}].type`,
        "must be public-key",
      );
    }
    if (
      typeof parameter.alg !== "number" || !Number.isSafeInteger(parameter.alg)
    ) {
      invalid(
        `public_key.publicKey.pubKeyCredParams[${index}].alg`,
        "must be a safe integer",
      );
    }
    return { type: "public-key" as const, alg: parameter.alg };
  });

  let excludeCredentials: PublicKeyCredentialDescriptor[] | undefined;
  if (options.excludeCredentials !== undefined) {
    if (!Array.isArray(options.excludeCredentials)) {
      invalid("public_key.publicKey.excludeCredentials", "must be an array");
    }
    if (options.excludeCredentials.length > MAX_AUTH_ARRAY_LENGTH) {
      invalid("public_key.publicKey.excludeCredentials", "has too many items");
    }
    excludeCredentials = options.excludeCredentials.map((descriptor, index) =>
      parseCredentialDescriptor(
        descriptor,
        `public_key.publicKey.excludeCredentials[${index}]`,
      )
    );
  }

  const timeout = options.timeout === undefined
    ? undefined
    : positiveSafeInteger(
      options.timeout,
      "public_key.publicKey.timeout",
      MAX_WEBAUTHN_TIMEOUT_MS,
    );

  return {
    ...(options as unknown as PublicKeyCredentialCreationOptions),
    rp: normalizedRp,
    user: normalizedUser,
    challenge: fromBase64Url(
      options.challenge,
      "public_key.publicKey.challenge",
    ),
    pubKeyCredParams,
    ...(excludeCredentials === undefined ? {} : { excludeCredentials }),
    ...(timeout === undefined ? {} : { timeout }),
  };
}

function parseRequestOptions(
  value: unknown,
): PublicKeyCredentialRequestOptions {
  const options = unwrapPublicKey(value, "public_key");
  if (!("challenge" in options)) {
    invalid("public_key.publicKey.challenge", "is required");
  }

  let allowCredentials: PublicKeyCredentialDescriptor[] | undefined;
  if (options.allowCredentials !== undefined) {
    if (!Array.isArray(options.allowCredentials)) {
      invalid("public_key.publicKey.allowCredentials", "must be an array");
    }
    if (options.allowCredentials.length > MAX_AUTH_ARRAY_LENGTH) {
      invalid("public_key.publicKey.allowCredentials", "has too many items");
    }
    allowCredentials = options.allowCredentials.map((descriptor, index) =>
      parseCredentialDescriptor(
        descriptor,
        `public_key.publicKey.allowCredentials[${index}]`,
      )
    );
  }
  const timeout = options.timeout === undefined
    ? undefined
    : positiveSafeInteger(
      options.timeout,
      "public_key.publicKey.timeout",
      MAX_WEBAUTHN_TIMEOUT_MS,
    );
  if (options.rpId !== undefined) {
    boundedString(options.rpId, "public_key.publicKey.rpId");
  }

  return {
    ...(options as unknown as PublicKeyCredentialRequestOptions),
    challenge: fromBase64Url(
      options.challenge,
      "public_key.publicKey.challenge",
    ),
    ...(allowCredentials === undefined ? {} : { allowCredentials }),
    ...(timeout === undefined ? {} : { timeout }),
  };
}

function parseAuthenticatedUser(
  value: unknown,
  path: string,
): AuthenticatedUser {
  const user = asRecord(value, path);
  requireExactKeys(user, path, [
    "user_id",
    "account_id",
    "handle",
    "display_name",
  ]);
  return {
    user_id: boundedString(user.user_id, `${path}.user_id`),
    account_id: boundedString(user.account_id, `${path}.account_id`),
    handle: boundedString(user.handle, `${path}.handle`),
    display_name: boundedString(user.display_name, `${path}.display_name`, {
      allowEmpty: true,
    }),
  };
}

function parseActor(value: unknown, path: string): RequestActor {
  const actor = asRecord(value, path);
  requireExactKeys(actor, path, [
    "user_id",
    "account_id",
    "handle",
    "display_name",
    "auth_method",
  ]);
  const authMethod = actor.auth_method;
  if (authMethod !== "browser_session" && authMethod !== "api_token") {
    invalid(`${path}.auth_method`, "contains an unknown value");
  }
  return {
    user_id: boundedString(actor.user_id, `${path}.user_id`),
    account_id: boundedString(actor.account_id, `${path}.account_id`),
    handle: boundedString(actor.handle, `${path}.handle`),
    display_name: boundedString(actor.display_name, `${path}.display_name`, {
      allowEmpty: true,
    }),
    auth_method: authMethod as ActorAuthMethod,
  };
}

export function parseWhoamiResponse(value: unknown): WhoamiResponse {
  const response = asRecord(value, "whoami");
  requireExactKeys(response, "whoami", ["actor"]);
  return {
    actor: response.actor === null
      ? null
      : parseActor(response.actor, "whoami.actor"),
  };
}

export function parseAuthPublicConfig(value: unknown): AuthPublicConfig {
  const response = asRecord(value, "auth_config");
  requireExactKeys(response, "auth_config", [
    "rp_id",
    "origin",
    "public_base_url",
    "cookie_name",
  ]);
  const result = {
    rp_id: boundedString(response.rp_id, "auth_config.rp_id"),
    origin: boundedString(response.origin, "auth_config.origin"),
    public_base_url: boundedString(
      response.public_base_url,
      "auth_config.public_base_url",
    ),
    cookie_name: boundedString(response.cookie_name, "auth_config.cookie_name"),
  };
  for (
    const [field, url] of [["origin", result.origin], [
      "public_base_url",
      result.public_base_url,
    ]]
  ) {
    try {
      const parsed = new URL(url);
      if (
        !(["http:", "https:"].includes(parsed.protocol)) || parsed.username ||
        parsed.password
      ) {
        invalid(`auth_config.${field}`, "must be a safe HTTP(S) URL");
      }
    } catch {
      invalid(`auth_config.${field}`, "must be a valid URL");
    }
  }
  return result;
}

export function parseAuthUserResponse(value: unknown): PasskeyUserResponse {
  const response = asRecord(value, "auth_user");
  requireExactKeys(response, "auth_user", ["user"]);
  return { user: parseAuthenticatedUser(response.user, "auth_user.user") };
}

export function prepareRegistrationOptions(
  value: unknown,
): PublicKeyCredentialCreationOptions {
  const response = asRecord(value, "registration_options");
  requireExactKeys(response, "registration_options", [
    "challenge_id",
    "public_key",
  ]);
  boundedString(response.challenge_id, "registration_options.challenge_id");
  return parseCreationOptions(response.public_key);
}

export function prepareLoginOptions(
  value: unknown,
): PublicKeyCredentialRequestOptions {
  const response = asRecord(value, "login_options");
  requireExactKeys(response, "login_options", ["challenge_id", "public_key"]);
  boundedString(response.challenge_id, "login_options.challenge_id");
  return parseRequestOptions(response.public_key);
}

function credentialCore(credential: unknown): Record<string, unknown> {
  const result = asRecord(credential, "credential");
  if (result.type !== "public-key") {
    invalid("credential.type", "must be public-key");
  }
  boundedString(result.id, "credential.id");
  toBase64Url(result.rawId, "credential.rawId");
  if (typeof result.getClientExtensionResults !== "function") {
    invalid("credential.getClientExtensionResults", "must be a function");
  }
  return result;
}

function clientExtensionResults(
  credential: Record<string, unknown>,
): AuthenticationExtensionsClientOutputs {
  const value = (credential.getClientExtensionResults as () => unknown).call(
    credential,
  );
  boundedJson(value, "credential.clientExtensionResults");
  return asRecord(
    value,
    "credential.clientExtensionResults",
  ) as AuthenticationExtensionsClientOutputs;
}

export function registrationCredentialToJson(credential: unknown): unknown {
  const core = credentialCore(credential);
  const response = asRecord(core.response, "credential.response");
  if (typeof response.getTransports !== "function") {
    invalid("credential.response.getTransports", "must be a function");
  }
  const transportsValue = (response.getTransports as () => unknown).call(
    response,
  );
  if (
    !Array.isArray(transportsValue) ||
    transportsValue.length > MAX_AUTH_ARRAY_LENGTH
  ) {
    invalid("credential.response.transports", "must be a bounded array");
  }
  const transports = transportsValue.map((transport, index) =>
    parseAuthenticatorTransport(
      transport,
      `credential.response.transports[${index}]`,
    )
  );
  const authenticatorAttachment = optionalString(
    core.authenticatorAttachment,
    "credential.authenticatorAttachment",
  );
  return {
    id: boundedString(core.id, "credential.id"),
    rawId: toBase64Url(core.rawId, "credential.rawId"),
    type: "public-key",
    response: {
      clientDataJSON: toBase64Url(
        response.clientDataJSON,
        "credential.response.clientDataJSON",
      ),
      attestationObject: toBase64Url(
        response.attestationObject,
        "credential.response.attestationObject",
      ),
      transports,
    },
    clientExtensionResults: clientExtensionResults(core),
    ...(authenticatorAttachment === null ? {} : { authenticatorAttachment }),
  };
}

export function authenticationCredentialToJson(credential: unknown): unknown {
  const core = credentialCore(credential);
  const response = asRecord(core.response, "credential.response");
  const authenticatorAttachment = optionalString(
    core.authenticatorAttachment,
    "credential.authenticatorAttachment",
  );
  return {
    id: boundedString(core.id, "credential.id"),
    rawId: toBase64Url(core.rawId, "credential.rawId"),
    type: "public-key",
    response: {
      clientDataJSON: toBase64Url(
        response.clientDataJSON,
        "credential.response.clientDataJSON",
      ),
      authenticatorData: toBase64Url(
        response.authenticatorData,
        "credential.response.authenticatorData",
      ),
      signature: toBase64Url(
        response.signature,
        "credential.response.signature",
      ),
      userHandle: response.userHandle === null
        ? null
        : toBase64Url(response.userHandle, "credential.response.userHandle"),
    },
    clientExtensionResults: clientExtensionResults(core),
    ...(authenticatorAttachment === null ? {} : { authenticatorAttachment }),
  };
}

export function parseDeviceLoginStartResponse(
  value: unknown,
): DeviceLoginStartResponse {
  const response = asRecord(value, "device_login_start");
  requireExactKeys(response, "device_login_start", [
    "device_code",
    "user_code",
    "verification_uri",
    "verification_uri_complete",
    "expires_in",
    "interval",
  ]);
  const result = {
    device_code: boundedString(
      response.device_code,
      "device_login_start.device_code",
    ),
    user_code: boundedString(
      response.user_code,
      "device_login_start.user_code",
    ),
    verification_uri: boundedString(
      response.verification_uri,
      "device_login_start.verification_uri",
    ),
    verification_uri_complete: boundedString(
      response.verification_uri_complete,
      "device_login_start.verification_uri_complete",
    ),
    expires_in: positiveSafeInteger(
      response.expires_in,
      "device_login_start.expires_in",
      MAX_DEVICE_LOGIN_SECONDS,
    ),
    interval: positiveSafeInteger(
      response.interval,
      "device_login_start.interval",
      60,
    ),
  };
  for (
    const [field, url] of [
      ["verification_uri", result.verification_uri],
      ["verification_uri_complete", result.verification_uri_complete],
    ]
  ) {
    try {
      const parsed = new URL(url);
      if (
        !(["http:", "https:"].includes(parsed.protocol)) || parsed.username ||
        parsed.password
      ) {
        invalid(`device_login_start.${field}`, "must be a safe HTTP(S) URL");
      }
    } catch {
      invalid(`device_login_start.${field}`, "must be a valid URL");
    }
  }
  return result;
}

export function parseDeviceLoginPollResponse(
  value: unknown,
): DeviceLoginPollResponse {
  const response = asRecord(value, "device_login_poll");
  requireExactKeys(response, "device_login_poll", ["status"], [
    "access_token",
    "token_type",
  ]);
  const status = response.status;
  if (
    !["pending", "approved", "expired", "denied", "consumed"].includes(
      String(status),
    )
  ) {
    invalid("device_login_poll.status", "contains an unknown value");
  }
  const typedStatus = status as DeviceLoginPollStatus;
  const accessToken = optionalString(
    response.access_token,
    "device_login_poll.access_token",
  );
  const tokenType = optionalString(
    response.token_type,
    "device_login_poll.token_type",
  );
  if (typedStatus === "approved") {
    if (accessToken === null || tokenType !== "Bearer") {
      invalid("device_login_poll", "has invalid approved token fields");
    }
  } else if (accessToken !== null || tokenType !== null) {
    invalid("device_login_poll", "has token fields for a non-approved status");
  }
  return {
    status: typedStatus,
    ...(accessToken === null ? {} : { access_token: accessToken }),
    ...(tokenType === null ? {} : { token_type: "Bearer" as const }),
  };
}

export function parseDeviceApprovalResponse(
  value: unknown,
): DeviceApprovalResponse {
  const response = asRecord(value, "device_login_approval");
  requireExactKeys(response, "device_login_approval", ["status", "user"]);
  if (response.status !== "approved") {
    invalid("device_login_approval.status", "contains an unknown value");
  }
  return {
    status: "approved",
    user: parseAuthenticatedUser(response.user, "device_login_approval.user"),
  };
}

export function parseLogoutResponse(value: unknown): LogoutResponse {
  const response = asRecord(value, "logout");
  requireExactKeys(response, "logout", ["status"]);
  if (response.status !== "logged_out") {
    invalid("logout.status", "contains an unknown value");
  }
  return { status: "logged_out" };
}
