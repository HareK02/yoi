// Generated from workspace-api. Do not edit by hand.
// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_auth_api_types > web/workspace/src/lib/generated/auth-api.ts

export type AuthPublicConfig = {
  rp_id: string;
  origin: string;
  public_base_url: string;
  cookie_name: string;
};

export type ActorAuthMethod = "browser_session" | "api_token";

export type AuthenticatedUser = {
  user_id: string;
  account_id: string;
  handle: string;
  display_name: string;
};

export type RequestActor = {
  user_id: string;
  account_id: string;
  handle: string;
  display_name: string;
  auth_method: ActorAuthMethod;
};

export type WhoamiResponse = { actor: RequestActor | null };

export type AuthBootstrapUserRequest = {
  handle: string;
  display_name?: string | null;
};

export type AuthUserResponse = { user: AuthenticatedUser };

export type PasskeyRegistrationOptionsRequest = {
  handle: string;
  display_name?: string | null;
  browser_origin?: string | null;
};

export type PasskeyRegistrationOptionsResponse = {
  challenge_id: string;
  public_key: unknown;
};

export type PasskeyRegistrationCompleteRequest = {
  challenge_id: string;
  credential: unknown;
};

export type PasskeyLoginOptionsRequest = {
  handle?: string | null;
  browser_origin?: string | null;
};

export type PasskeyLoginOptionsResponse = {
  challenge_id: string;
  public_key: unknown;
};

export type PasskeyLoginCompleteRequest = {
  challenge_id: string;
  credential: unknown;
};

export type DeviceLoginStartRequest = { client_name?: string | null };

export type DeviceLoginStartResponse = {
  device_code: string;
  user_code: string;
  verification_uri: string;
  verification_uri_complete: string;
  expires_in: number;
  interval: number;
};

export type DeviceLoginApproveRequest = { user_code: string };

export type DeviceLoginApprovalStatus = "approved";

export type DeviceLoginApproveResponse = {
  status: DeviceLoginApprovalStatus;
  user: AuthenticatedUser;
};

export type DeviceLoginPollRequest = { device_code: string };

export type DeviceAccessTokenType = "Bearer";

export type DeviceLoginPollStatus =
  | "pending"
  | "approved"
  | "expired"
  | "consumed";

export type DeviceLoginPollResponse = {
  status: DeviceLoginPollStatus;
  access_token?: string | null;
  token_type?: DeviceAccessTokenType | null;
};

export type LogoutStatus = "logged_out";

export type LogoutResponse = { status: LogoutStatus };
