//! 隔離した実yoi-serverのhost CLI・HTTP APIで、管理者初期設定、登録可否の永続化、
//! 明示handle login、本人の鍵追加、Workspace所有境界を検証する。DB・認証・routerは本番実装を使い、
//! authenticatorだけをSoftPasskeyに置換する（browser UI／OS authenticatorは対象外）。
//! `cargo test -p yoi-e2e --features e2e --test auth_passkeys`で明示選択する。
//! binary供給、HOME/XDG/DB/動的port、readiness、ログ、RAII回収は共有harnessが所有する。
//! HTTPはproxy／redirect／cookie storeなし、各要求にtimeoutを設ける。

use std::error::Error;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{COOKIE, ORIGIN, SET_COOKIE};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use server_api::{
    AuthUserResponse, PasskeyLoginCompleteRequest, PasskeyLoginOptionsResponse,
    PasskeyRegistrationCompleteRequest, PasskeyRegistrationOptionsResponse,
};
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
use yoi_e2e::WorkerE2eEnvironment;

type Result<T = ()> = std::result::Result<T, Box<dyn Error>>;
const BROWSER_ORIGIN: &str = "http://localhost:5173";

// 通信と診断だけを補助する。起動／DB／process管理は共有harnessに任せる。
fn send(request: RequestBuilder, operation: &str, artifacts: &Path) -> Result<Response> {
    let response = request.send()?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(artifacts.join("auth-operations.jsonl"))?;
    writeln!(
        log,
        "{}",
        json!({"operation": operation, "status": response.status().as_u16()})
    )?;
    Ok(response)
}

fn success(response: Response) -> Result<Response> {
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "unexpected {status}: {}",
            response.text()?.chars().take(2048).collect::<String>()
        )
        .into());
    }
    Ok(response)
}

fn decode<T: DeserializeOwned>(response: Response) -> Result<T> {
    Ok(success(response)?.json()?)
}

fn rejected(response: Response, expected: StatusCode, diagnostic: &str) -> Result {
    let status = response.status();
    let body = response.text()?;
    assert!(
        status == expected,
        "expected rejection {expected}, got {status}: {}",
        body.chars().take(2048).collect::<String>()
    );
    assert!(
        body.contains(diagnostic),
        "missing diagnostic {diagnostic:?}: {}",
        body.chars().take(2048).collect::<String>()
    );
    Ok(())
}

fn session(response: Response) -> Result<(AuthUserResponse, String)> {
    let response = success(response)?;
    let header = response
        .headers()
        .get(SET_COOKIE)
        .ok_or("missing session cookie")?
        .to_str()?;
    assert!(header.contains("HttpOnly"));
    assert!(header.contains("SameSite=Lax"));
    let cookie = header
        .split(';')
        .next()
        .ok_or("missing cookie pair")?
        .to_owned();
    Ok((response.json()?, cookie))
}

fn register(
    client: &Client,
    base: &str,
    artifacts: &Path,
    handle: &str,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
) -> Result<(AuthUserResponse, String, Value)> {
    let options: PasskeyRegistrationOptionsResponse = decode(send(
        client.post(format!("{base}/api/auth/passkeys/registration/options"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"handle": handle, "display_name": "Isolated passkey E2E", "browser_origin": BROWSER_ORIGIN})),
        "new account registration options", artifacts,
    )?)?;
    let user_handle = serde_json::to_value(&options.public_key)?["publicKey"]["user"]["id"].clone();
    assert!(user_handle.is_string(), "WebAuthn user handle is absent");
    let credential = authenticator.do_registration(BROWSER_ORIGIN.parse()?, options.public_key)?;
    let (user, cookie) = session(send(
        client
            .post(format!("{base}/api/auth/passkeys/registration/complete"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&PasskeyRegistrationCompleteRequest {
                challenge_id: options.challenge_id,
                credential,
            }),
        "new account registration complete",
        artifacts,
    )?)?;
    assert_eq!(user.user.handle, handle);
    Ok((user, cookie, user_handle))
}

fn login(
    client: &Client,
    base: &str,
    artifacts: &Path,
    handle: &str,
    authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
) -> Result<(AuthUserResponse, String)> {
    let options: PasskeyLoginOptionsResponse = decode(send(
        client
            .post(format!("{base}/api/auth/passkeys/login/options"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"handle": handle, "browser_origin": BROWSER_ORIGIN})),
        "explicit handle login options",
        artifacts,
    )?)?;
    let credential =
        authenticator.do_authentication(BROWSER_ORIGIN.parse()?, options.public_key)?;
    session(send(
        client
            .post(format!("{base}/api/auth/passkeys/login/complete"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&PasskeyLoginCompleteRequest {
                challenge_id: options.challenge_id,
                credential,
            }),
        "verified login complete",
        artifacts,
    )?)
}

fn http_client() -> Result<Client> {
    Ok(Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(5))
        .build()?)
}

fn policy(client: &Client, base: &str, artifacts: &Path, enabled: bool) -> Result {
    let public: Value = decode(send(
        client.get(format!("{base}/api/auth/registration-policy")),
        "anonymous public registration policy",
        artifacts,
    )?)?;
    assert_eq!(public, json!({"self_registration_enabled": enabled}));
    Ok(())
}

fn set_policy(
    client: &Client,
    base: &str,
    artifacts: &Path,
    cookie: &str,
    enabled: bool,
) -> Result {
    let updated: Value = decode(send(
        client
            .put(format!("{base}/api/hosting/settings"))
            .header(COOKIE, cookie)
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"self_registration_enabled": enabled})),
        "administrator updates registration policy",
        artifacts,
    )?)?;
    assert_eq!(updated, json!({"self_registration_enabled": enabled}));
    let stored: Value = decode(send(
        client
            .get(format!("{base}/api/hosting/settings"))
            .header(COOKIE, cookie),
        "administrator rereads stored hosting settings",
        artifacts,
    )?)?;
    assert_eq!(stored, updated);
    policy(client, base, artifacts, enabled)
}

fn access(client: &Client, base: &str, artifacts: &Path, cookie: &str, is_admin: bool) -> Result {
    let value: Value = decode(send(
        client
            .get(format!("{base}/api/hosting/access"))
            .header(COOKIE, cookie),
        "authenticated hosting access",
        artifacts,
    )?)?;
    assert_eq!(value, json!({"is_admin": is_admin}));
    Ok(())
}

fn logout(client: &Client, base: &str, artifacts: &Path, cookie: &str) -> Result {
    let response = success(send(
        client
            .post(format!("{base}/api/auth/logout"))
            .header(COOKIE, cookie)
            .header(ORIGIN, BROWSER_ORIGIN),
        "revoke real browser session",
        artifacts,
    )?)?;
    assert!(
        response
            .headers()
            .get(SET_COOKIE)
            .ok_or("missing logout cookie")?
            .to_str()?
            .contains("Max-Age=0")
    );
    let whoami: Value = decode(send(
        client
            .get(format!("{base}/api/auth/whoami"))
            .header(COOKIE, cookie),
        "revoked cookie is no longer authenticated",
        artifacts,
    )?)?;
    assert!(whoami["actor"].is_null());
    Ok(())
}

fn host_admin_login(
    environment: &WorkerE2eEnvironment,
    client: &Client,
    key: &mut WebauthnAuthenticator<SoftPasskey>,
) -> Result<(AuthUserResponse, String)> {
    let base = environment
        .server_url
        .as_deref()
        .ok_or("missing server URL")?;
    let artifacts = &environment.artifacts_dir;
    let handle = format!("{}-admin", environment.account_handle);
    let setup = environment.setup_hosting_admin(&handle, true)?;
    assert_eq!(setup["hosting_admin"], true);
    assert_eq!(setup["token_type"], "bearer");
    assert!(
        setup["expires_at"].as_str().is_some(),
        "initial bearer must have finite expiry"
    );
    let bearer = setup["access_token"]
        .as_str()
        .ok_or("host setup omitted initial bearer")?;
    let options: PasskeyRegistrationOptionsResponse = decode(send(
        client
            .post(format!("{base}/api/auth/passkeys/addition/options"))
            .bearer_auth(bearer)
            .json(&json!({"browser_origin": BROWSER_ORIGIN})),
        "host-created administrator bearer starts actor-derived passkey addition",
        artifacts,
    )?)?;
    let credential = key.do_registration(BROWSER_ORIGIN.parse()?, options.public_key)?;
    let (activated, cookie) = session(send(
        client
            .post(format!("{base}/api/auth/passkeys/addition/complete"))
            .bearer_auth(bearer)
            .json(&PasskeyRegistrationCompleteRequest {
                challenge_id: options.challenge_id,
                credential,
            }),
        "host-created administrator verifies initial passkey",
        artifacts,
    )?)?;
    assert_eq!(activated.user.account_id, setup["user"]["account_id"]);
    logout(client, base, artifacts, &cookie)?;
    let (logged_in, cookie) = login(client, base, artifacts, &handle, key)?;
    assert_eq!(logged_in.user, activated.user);
    access(client, base, artifacts, &cookie, true)?;
    let workspaces: Value = decode(send(
        client
            .get(format!("{base}/api/workspaces"))
            .header(COOKIE, &cookie),
        "host administrator setup and login do not create a Workspace",
        artifacts,
    )?)?;
    assert_eq!(workspaces, json!([]));
    Ok((logged_in, cookie))
}

#[test]
fn disabled_registration_preserves_existing_login_self_addition_and_workspace_ownership() -> Result
{
    let mut environment = WorkerE2eEnvironment::new()?;
    eprintln!(
        "passkey E2E artifacts: {}",
        environment.artifacts_dir.display()
    );
    assert!(!environment.server_data_dir.exists());
    environment.start_empty_backend()?;
    assert!(environment.workspace_id.is_none());
    assert!(environment.runtime_pid().is_none());
    let base = environment
        .server_url
        .as_deref()
        .ok_or("missing server URL")?;
    let artifacts = &environment.artifacts_dir;
    let client = http_client()?;
    policy(&client, base, artifacts, false)?;
    let mut admin_key = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (admin, admin_cookie) = host_admin_login(&environment, &client, &mut admin_key)?;
    policy(&client, base, artifacts, false)?;
    set_policy(&client, base, artifacts, &admin_cookie, true)?;
    let whoami: Value = decode(send(
        client.get(format!("{base}/api/auth/whoami")),
        "fresh whoami",
        artifacts,
    )?)?;
    assert!(whoami["actor"].is_null());

    let workspace_request = json!({
        "operation_key": format!("passkey-e2e-{}", environment.runtime_id),
        "display_name": "Passkey owner-first E2E",
        "repository": {"repository_key": "fixture", "uri": "https://example.invalid/passkey-e2e.git", "default_ref": "develop"}
    });
    rejected(
        send(
            client
                .post(format!("{base}/api/workspaces"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&workspace_request),
            "unauthenticated loopback cannot bootstrap Workspace",
            artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "authenticated owner",
    )?;
    rejected(
        send(
            client
                .post(format!("{base}/api/auth/passkeys/addition/options"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"browser_origin": BROWSER_ORIGIN})),
            "unauthenticated addition options",
            artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "auth_required",
    )?;

    let mut original_key = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (registered, _, user_handle) = register(
        &client,
        base,
        artifacts,
        &environment.account_handle,
        &mut original_key,
    )?;
    // registrationのcookieは使わず、実assertionで得たlogin cookieを次の変更操作に使う。
    let (logged_in, cookie) = login(
        &client,
        base,
        artifacts,
        &environment.account_handle,
        &mut original_key,
    )?;
    assert_eq!(logged_in.user, registered.user);
    let whoami: Value = decode(send(
        client
            .get(format!("{base}/api/auth/whoami"))
            .header(COOKIE, &cookie),
        "logged-in whoami",
        artifacts,
    )?)?;
    assert_eq!(whoami["actor"]["account_id"], registered.user.account_id);
    let workspaces: Value = decode(send(
        client
            .get(format!("{base}/api/workspaces"))
            .header(COOKIE, &cookie),
        "zero Workspaces after registration and login",
        artifacts,
    )?)?;
    assert_eq!(
        workspaces,
        json!([]),
        "startup/registration must not create a Workspace"
    );
    rejected(send(
        client.post(format!("{base}/api/auth/passkeys/registration/options"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"handle": environment.account_handle, "display_name": "Attempted replacement", "browser_origin": BROWSER_ORIGIN})),
        "existing handle cannot use unauthenticated registration", artifacts,
    )?, StatusCode::BAD_REQUEST, "auth_handle_exists")?;

    // 2 usersを実登録し、handle省略が最古userへfallbackしないことを検証する。
    let mut other_key = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let other_handle = format!("{}-other", environment.account_handle);
    let (other, _, other_user_handle) =
        register(&client, base, artifacts, &other_handle, &mut other_key)?;
    assert_ne!(other.user.account_id, registered.user.account_id);
    assert_ne!(other_user_handle, user_handle);
    access(&client, base, artifacts, &cookie, false)?;
    set_policy(&client, base, artifacts, &admin_cookie, false)?;
    // 以下の本人鍵追加と両方の鍵によるloginは、自己登録停止中にも必要。
    rejected(
        send(
            client
                .post(format!("{base}/api/auth/passkeys/login/options"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"browser_origin": BROWSER_ORIGIN})),
            "omitted handle cannot select first account",
            artifacts,
        )?,
        StatusCode::BAD_REQUEST,
        "passkey_handle_required",
    )?;

    for (field, value) in [
        ("handle", &other.user.handle),
        ("user_id", &other.user.user_id),
        ("account_id", &other.user.account_id),
    ] {
        let mut body = json!({"browser_origin": BROWSER_ORIGIN});
        body[field] = json!(value);
        rejected(
            send(
                client
                    .post(format!("{base}/api/auth/passkeys/addition/options"))
                    .header(COOKIE, &cookie)
                    .header(ORIGIN, BROWSER_ORIGIN)
                    .json(&body),
                &format!("reject addition target injection: {field}"),
                artifacts,
            )?,
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown field",
        )?;
    }
    for origin in [None, Some("http://localhost:5174")] {
        let mut request = client
            .post(format!("{base}/api/auth/passkeys/addition/options"))
            .header(COOKIE, &cookie)
            .json(&json!({"browser_origin": BROWSER_ORIGIN}));
        if let Some(origin) = origin {
            request = request.header(ORIGIN, origin);
        }
        rejected(
            send(
                request,
                "cookie addition requires actual matching Origin header",
                artifacts,
            )?,
            StatusCode::FORBIDDEN,
            "origin",
        )?;
    }

    let options: PasskeyRegistrationOptionsResponse = decode(send(
        client
            .post(format!("{base}/api/auth/passkeys/addition/options"))
            .header(COOKIE, &cookie)
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"browser_origin": BROWSER_ORIGIN})),
        "authenticated self addition options",
        artifacts,
    )?)?;
    assert_eq!(
        serde_json::to_value(&options.public_key)?["publicKey"]["user"]["id"],
        user_handle,
        "same account must retain its WebAuthn user handle"
    );
    let mut added_key = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = added_key.do_registration(BROWSER_ORIGIN.parse()?, options.public_key)?;
    let complete = PasskeyRegistrationCompleteRequest {
        challenge_id: options.challenge_id,
        credential,
    };
    rejected(
        send(
            client
                .post(format!("{base}/api/auth/passkeys/addition/complete"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&complete),
            "unauthenticated addition complete",
            artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "auth_required",
    )?;
    for origin in [None, Some("http://localhost:5174")] {
        let mut request = client
            .post(format!("{base}/api/auth/passkeys/addition/complete"))
            .header(COOKIE, &cookie)
            .json(&complete);
        if let Some(origin) = origin {
            request = request.header(ORIGIN, origin);
        }
        rejected(
            send(
                request,
                "cookie addition complete requires matching Origin",
                artifacts,
            )?,
            StatusCode::FORBIDDEN,
            "origin",
        )?;
    }
    for (field, value) in [
        ("handle", &other.user.handle),
        ("user_id", &other.user.user_id),
        ("account_id", &other.user.account_id),
    ] {
        let mut body = serde_json::to_value(&complete)?;
        body[field] = json!(value);
        rejected(
            send(
                client
                    .post(format!("{base}/api/auth/passkeys/addition/complete"))
                    .header(COOKIE, &cookie)
                    .header(ORIGIN, BROWSER_ORIGIN)
                    .json(&body),
                &format!("reject addition complete target injection: {field}"),
                artifacts,
            )?,
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown field",
        )?;
    }
    let (added, _) = session(send(
        client
            .post(format!("{base}/api/auth/passkeys/addition/complete"))
            .header(COOKIE, &cookie)
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&complete),
        "authenticated self addition complete",
        artifacts,
    )?)?;
    assert_eq!(added.user, registered.user);
    rejected(
        send(
            client
                .post(format!("{base}/api/auth/passkeys/addition/complete"))
                .header(COOKIE, &cookie)
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&complete),
            "addition challenge cannot be replayed",
            artifacts,
        )?,
        StatusCode::BAD_REQUEST,
        "invalid_passkey_challenge",
    )?;

    let (new_login, new_cookie) = login(
        &client,
        base,
        artifacts,
        &environment.account_handle,
        &mut added_key,
    )?;
    assert_eq!(new_login.user, registered.user);
    let (old_login, _) = login(
        &client,
        base,
        artifacts,
        &environment.account_handle,
        &mut original_key,
    )?;
    assert_eq!(
        old_login.user, registered.user,
        "adding a key must preserve the original key"
    );
    let created: Value = decode(send(
        client
            .post(format!("{base}/api/workspaces"))
            .header(COOKIE, &new_cookie)
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&workspace_request),
        "first Workspace from verified new-key login",
        artifacts,
    )?)?;
    assert_eq!(
        created["workspace"]["owner_account_id"],
        registered.user.account_id
    );
    let workspace_id = created["workspace"]["workspace_id"]
        .as_str()
        .ok_or("missing Workspace ID")?;
    let listed: Value = decode(send(
        client
            .get(format!("{base}/api/workspaces"))
            .header(COOKIE, &new_cookie),
        "reread owned Workspace",
        artifacts,
    )?)?;
    let items = listed
        .as_array()
        .ok_or("Workspace catalog is not an array")?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["workspace_id"], workspace_id);
    assert_eq!(items[0]["owner_account_id"], registered.user.account_id);

    access(&client, base, artifacts, &new_cookie, false)?;
    rejected(
        send(
            client
                .get(format!("{base}/api/hosting/settings"))
                .header(COOKIE, &new_cookie),
            "Workspace ownership does not grant hosting administrator access",
            artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "hosting administrator required",
    )?;
    let admin_workspaces: Value = decode(send(
        client
            .get(format!("{base}/api/workspaces"))
            .header(COOKIE, &admin_cookie),
        "hosting administrator cannot list another owner's Workspace",
        artifacts,
    )?)?;
    assert_eq!(admin_workspaces, json!([]));
    assert_ne!(admin.user.account_id, registered.user.account_id);
    rejected(
        send(
            client
                .get(format!("{base}/api/workspaces/{workspace_id}/deletion"))
                .header(COOKIE, &admin_cookie),
            "hosting administrator cannot manage another owner's Workspace deletion",
            artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "owner",
    )?;

    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    assert!(!report.fixture_root_exists_after);
    Ok(())
}

fn ordinary_device_login_does_not_grant_hosting_authority(
    client: &Client,
    base: &str,
    artifacts: &Path,
    user: &AuthUserResponse,
    cookie: &str,
) -> Result {
    let flow: Value = decode(send(
        client
            .post(format!("{base}/api/auth/device-login/start"))
            .json(&json!({"client_name": "isolated policy E2E"})),
        "start device login while registration is disabled",
        artifacts,
    )?)?;
    let approval = json!({"user_code": flow["user_code"]});
    rejected(
        send(
            client
                .post(format!("{base}/api/auth/device-login/approve"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&approval),
            "anonymous device approval cannot authenticate",
            artifacts,
        )?,
        StatusCode::UNAUTHORIZED,
        "request requires",
    )?;
    for origin in [None, Some("http://localhost:5174")] {
        let mut request = client
            .post(format!("{base}/api/auth/device-login/approve"))
            .header(COOKIE, cookie)
            .json(&approval);
        if let Some(origin) = origin {
            request = request.header(ORIGIN, origin);
        }
        rejected(
            send(
                request,
                "device approval cookie retains Origin boundary",
                artifacts,
            )?,
            StatusCode::FORBIDDEN,
            "origin",
        )?;
    }
    let approved: Value = decode(send(
        client
            .post(format!("{base}/api/auth/device-login/approve"))
            .header(COOKIE, cookie)
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&approval),
        "existing ordinary user approves device login while registration disabled",
        artifacts,
    )?)?;
    assert_eq!(approved["status"], "approved");
    assert_eq!(approved["user"]["account_id"], user.user.account_id);
    let issued: Value = decode(send(
        client
            .post(format!("{base}/api/auth/device-login/poll"))
            .json(&json!({"device_code": flow["device_code"]})),
        "device poll returns approved ordinary user's bearer",
        artifacts,
    )?)?;
    assert_eq!(issued["status"], "approved");
    let token = issued["access_token"]
        .as_str()
        .ok_or("device poll omitted bearer")?;
    let access: Value = decode(send(
        client
            .get(format!("{base}/api/hosting/access"))
            .bearer_auth(token),
        "device bearer retains ordinary hosting access",
        artifacts,
    )?)?;
    assert_eq!(access, json!({"is_admin": false}));
    rejected(
        send(
            client
                .put(format!("{base}/api/hosting/settings"))
                .bearer_auth(token)
                .header("x-hosting-admin", "true")
                .json(&json!({"self_registration_enabled": true})),
            "device bearer cannot elevate ordinary user to hosting administrator",
            artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "hosting_admin_required",
    )?;
    let consumed: Value = decode(send(
        client
            .post(format!("{base}/api/auth/device-login/poll"))
            .json(&json!({"device_code": flow["device_code"]})),
        "device credential delivery remains one-use",
        artifacts,
    )?)?;
    assert_eq!(consumed["status"], "consumed");
    assert!(consumed.get("access_token").is_none_or(Value::is_null));
    policy(client, base, artifacts, false)
}

#[test]
fn hosting_policy_rejects_spoofing_inflight_registration_and_survives_restart() -> Result {
    let mut environment = WorkerE2eEnvironment::new()?;
    eprintln!(
        "hosting policy E2E artifacts: {}",
        environment.artifacts_dir.display()
    );
    environment.start_empty_backend()?;
    let client = http_client()?;
    let mut base = environment.server_url.clone().ok_or("missing server URL")?;
    let artifacts = environment.artifacts_dir.clone();
    policy(&client, &base, &artifacts, false)?;

    let mut admin_key = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (admin, admin_cookie) = host_admin_login(&environment, &client, &mut admin_key)?;
    policy(&client, &base, &artifacts, false)?;
    for path in ["/api/hosting/access", "/api/hosting/settings"] {
        rejected(
            send(
                client
                    .get(format!("{base}{path}"))
                    .header("x-yoi-user-id", &admin.user.user_id)
                    .header("x-yoi-account-id", &admin.user.account_id)
                    .header("x-hosting-admin", "true"),
                "anonymous identity headers cannot authenticate hosting access",
                &artifacts,
            )?,
            StatusCode::FORBIDDEN,
            "auth_required",
        )?;
    }
    rejected(
        send(
            client
                .put(format!("{base}/api/hosting/settings"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .header("x-hosting-admin", "true")
                .json(&json!({"self_registration_enabled": true})),
            "anonymous spoof cannot enable registration",
            &artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "auth_required",
    )?;
    policy(&client, &base, &artifacts, false)?;

    let denied_handle = format!("{}-denied", environment.account_handle);
    rejected(
        send(
            client
                .post(format!("{base}/api/auth/passkeys/registration/options"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"handle": denied_handle, "browser_origin": BROWSER_ORIGIN})),
            "disabled public registration rejects direct options",
            &artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "self_registration_disabled",
    )?;
    rejected(
        send(
            client
                .post(format!("{base}/api/auth/bootstrap-user"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"handle": denied_handle})),
            "disabled policy cannot be bypassed through legacy bootstrap",
            &artifacts,
        )?,
        StatusCode::BAD_REQUEST,
        "verified_registration_required",
    )?;

    for origin in [None, Some("http://localhost:5174")] {
        let mut request = client
            .put(format!("{base}/api/hosting/settings"))
            .header(COOKIE, &admin_cookie)
            .json(&json!({"self_registration_enabled": true}));
        if let Some(origin) = origin {
            request = request.header(ORIGIN, origin);
        }
        rejected(
            send(
                request,
                "admin cookie settings require configured Origin",
                &artifacts,
            )?,
            StatusCode::FORBIDDEN,
            "origin",
        )?;
    }
    policy(&client, &base, &artifacts, false)?;
    set_policy(&client, &base, &artifacts, &admin_cookie, true)?;
    environment.restart_backend()?;
    base = environment
        .server_url
        .clone()
        .ok_or("missing restarted server URL")?;
    policy(&client, &base, &artifacts, true)?;
    let (_, admin_cookie) = login(
        &client,
        &base,
        &artifacts,
        &admin.user.handle,
        &mut admin_key,
    )?;
    access(&client, &base, &artifacts, &admin_cookie, true)?;

    // 拒否されたhandleも予約されておらず、許可後には通常の新規登録ができる。
    let mut ordinary_key = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (ordinary, ordinary_cookie, _) = register(
        &client,
        &base,
        &artifacts,
        &denied_handle,
        &mut ordinary_key,
    )?;
    access(&client, &base, &artifacts, &ordinary_cookie, false)?;
    rejected(
        send(
            client
                .get(format!("{base}/api/hosting/settings"))
                .header(COOKIE, &ordinary_cookie)
                .header("x-hosting-admin", "true")
                .header("x-yoi-user-id", &admin.user.user_id)
                .header("x-yoi-account-id", &admin.user.account_id),
            "ordinary user cannot read settings by impersonating administrator headers",
            &artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "hosting administrator required",
    )?;
    rejected(
        send(
            client
                .put(format!("{base}/api/hosting/settings"))
                .header(COOKIE, &ordinary_cookie)
                .header(ORIGIN, BROWSER_ORIGIN)
                .header("x-hosting-admin", "true")
                .header("x-yoi-user-id", &admin.user.user_id)
                .json(&json!({"self_registration_enabled": false})),
            "ordinary user cannot change policy with administrator headers",
            &artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "hosting_admin_required",
    )?;
    for field in ["is_admin", "user_id", "account_id"] {
        let mut body = json!({"self_registration_enabled": false});
        body[field] = match field {
            "is_admin" => json!(true),
            "user_id" => json!(admin.user.user_id),
            _ => json!(admin.user.account_id),
        };
        rejected(
            send(
                client
                    .put(format!("{base}/api/hosting/settings"))
                    .header(COOKIE, &ordinary_cookie)
                    .header(ORIGIN, BROWSER_ORIGIN)
                    .json(&body),
                "hosting settings reject actor and role fields in JSON",
                &artifacts,
            )?,
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown field",
        )?;
    }
    policy(&client, &base, &artifacts, true)?;

    let pending_handle = format!("{}-inflight", environment.account_handle);
    let options: PasskeyRegistrationOptionsResponse = decode(send(
        client
            .post(format!("{base}/api/auth/passkeys/registration/options"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"handle": pending_handle, "browser_origin": BROWSER_ORIGIN})),
        "registration starts while enabled",
        &artifacts,
    )?)?;
    let mut pending_key = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let credential = pending_key.do_registration(BROWSER_ORIGIN.parse()?, options.public_key)?;
    let complete = PasskeyRegistrationCompleteRequest {
        challenge_id: options.challenge_id,
        credential,
    };
    set_policy(&client, &base, &artifacts, &admin_cookie, false)?;
    // challenge自体も実DBから再読させ、停止後の完了で現在の設定を使うことを確かめる。
    environment.restart_backend()?;
    base = environment
        .server_url
        .clone()
        .ok_or("missing restarted server URL")?;
    policy(&client, &base, &artifacts, false)?;
    rejected(
        send(
            client
                .post(format!("{base}/api/auth/passkeys/registration/complete"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&complete),
            "registration completion rechecks disabled policy after restart",
            &artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "self_registration_disabled",
    )?;
    rejected(
        send(
            client
                .post(format!("{base}/api/auth/passkeys/login/options"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"handle": pending_handle, "browser_origin": BROWSER_ORIGIN})),
            "rejected in-flight registration did not create an account",
            &artifacts,
        )?,
        StatusCode::BAD_REQUEST,
        "unknown_auth_user",
    )?;
    rejected(
        send(
            client
                .post(format!("{base}/api/auth/passkeys/registration/options"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"handle": pending_handle, "browser_origin": BROWSER_ORIGIN})),
            "registration remains disabled on reopened DB",
            &artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "self_registration_disabled",
    )?;
    let (logged_in, cookie) = login(
        &client,
        &base,
        &artifacts,
        &ordinary.user.handle,
        &mut ordinary_key,
    )?;
    assert_eq!(logged_in.user, ordinary.user);
    access(&client, &base, &artifacts, &cookie, false)?;
    ordinary_device_login_does_not_grant_hosting_authority(
        &client, &base, &artifacts, &logged_in, &cookie,
    )?;
    let (_, cookie) = login(
        &client,
        &base,
        &artifacts,
        &admin.user.handle,
        &mut admin_key,
    )?;
    access(&client, &base, &artifacts, &cookie, true)?;
    let stored: Value = decode(send(
        client
            .get(format!("{base}/api/hosting/settings"))
            .header(COOKIE, &cookie),
        "administrator settings access persists after reopen",
        &artifacts,
    )?)?;
    assert_eq!(stored, json!({"self_registration_enabled": false}));
    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    assert!(!report.fixture_root_exists_after);
    Ok(())
}

#[test]
fn legacy_host_bootstrap_is_ordinary_until_explicit_host_selection() -> Result {
    let mut environment = WorkerE2eEnvironment::new()?;
    environment.start_empty_backend()?;
    let client = http_client()?;
    let base = environment
        .server_url
        .as_deref()
        .ok_or("missing server URL")?;
    let artifacts = &environment.artifacts_dir;
    let setup = environment.bootstrap_auth_user(&environment.account_handle)?;
    let bearer = setup["access_token"]
        .as_str()
        .ok_or("bootstrap omitted bearer")?;
    policy(&client, base, artifacts, false)?;
    let ordinary: Value = decode(send(
        client
            .get(format!("{base}/api/hosting/access"))
            .bearer_auth(bearer),
        "legacy host bootstrap does not implicitly grant administrator",
        artifacts,
    )?)?;
    assert_eq!(ordinary, json!({"is_admin": false}));
    rejected(
        send(
            client
                .put(format!("{base}/api/hosting/settings"))
                .bearer_auth(bearer)
                .json(&json!({"self_registration_enabled": true})),
            "legacy bootstrap bearer cannot change hosting settings",
            artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "hosting_admin_required",
    )?;
    let selected = environment.setup_hosting_admin(&environment.account_handle, false)?;
    assert_eq!(selected["hosting_admin"], true);
    assert!(
        selected.get("access_token").is_none(),
        "selection must not mint credentials"
    );
    let admin: Value = decode(send(
        client
            .get(format!("{base}/api/hosting/access"))
            .bearer_auth(bearer),
        "explicit host selection grants existing user administrator",
        artifacts,
    )?)?;
    assert_eq!(admin, json!({"is_admin": true}));
    policy(&client, base, artifacts, false)?;
    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    assert!(!report.fixture_root_exists_after);
    Ok(())
}
