# Backend account model and login flows

This document records the first Backend identity/auth layer introduced for the multi-workspace Backend direction.

## Account and owner model

The Backend store now separates a human user from the owner namespace used by workspace ownership:

- `accounts`: canonical namespace records. `kind = "user"` is implemented now; `kind = "organization"` is reserved in the schema for a later organization model.
- `users`: human user records. Each user owns exactly one `accounts` row.
- `workspaces.owner_account_id`: optional account owner reference. Existing/local workspaces may keep it unset until account bootstrap/ownership migration code assigns it.

This avoids baking `owner_user_id` into workspace authority while keeping the current implementation small.

## Browser login

Human browser auth is modeled as Passkey/WebAuthn ceremony endpoints:

- `POST /api/auth/bootstrap-user`
- `POST /api/auth/passkeys/registration/options`
- `POST /api/auth/passkeys/registration/complete`
- `POST /api/auth/passkeys/login/options`
- `POST /api/auth/passkeys/login/complete`
- `GET /api/auth/whoami`

The store persists registration/login challenges, serialized WebAuthn ceremony state, verified passkey credentials, browser sessions, and challenge consumption. Successful login mints an HttpOnly `SameSite=Lax` cookie session.

Completion endpoints require browser `PublicKeyCredential` responses and verify them with `webauthn-rs` before a passkey is stored or a browser session is issued. Fake or stale credential responses are rejected by the verifier, and consumed challenges cannot be reused.

## CLI/TUI device login

CLI/TUI clients use a device-login flow instead of browser cookies:

- `POST /api/auth/device-login/start` creates a `device_code` and human `user_code`.
- The CLI prints `verification_uri_complete` and polls `POST /api/auth/device-login/poll`.
- A browser-authenticated user approves with `POST /api/auth/device-login/approve`.
- Approval mints a Backend API token and returns it exactly once to the polling client.

`yoi login [--backend <URL>] [--no-wait]` starts this flow and stores the received token in `$XDG_CONFIG_HOME/yoi/backend-tokens.json` or `$HOME/.config/yoi/backend-tokens.json`.

## Request actor extraction

`workspace-server::auth::resolve_request_actor` resolves a typed request actor from either:

- `Authorization: Bearer <token>` for CLI/TUI API tokens, or
- the configured browser session cookie.

The resolved actor includes `user_id`, `account_id`, `handle`, `display_name`, and `auth_method`. This helper is intended to be used by mutating Backend APIs as authorization gets added route-by-route.

## Non-goals for this step

- Organization membership and RBAC.
- Password login, OAuth login, or other human auth methods.
- Token scoping beyond Backend API bearer authentication.
