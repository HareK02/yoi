//! 隔離した実yoi-serverのhost CLI・HTTP APIで、管理者初期設定、登録可否の永続化、
//! ID不要login、本人の鍵追加、限定activation、Workspace所有・catalog選択境界を検証する。DB・認証・routerは本番実装を使い、
//! authenticatorはresident store/選択を持つsoftware model、署名エンジンのみSoftPasskeyとする（OS／物理認証器は対象外）。
//! `cargo test -p yoi-e2e --features e2e --test auth_passkeys`で明示選択する。
//! binary供給、HOME/XDG/DB/動的port、readiness、ログ、RAII回収は共有harnessが所有する。
//! 通常ケースのHTTPはproxy／redirect／cookie storeなし、各要求にtimeoutを設ける。
//! 実Server CLIにはstatic assets設定経路がない。明示選択browser_sessionsは本番static shellを
//! loopback proxyで実Backendへ接続し、Cookieの自然消失と復帰を検証する。
//! native_migrationは同じshellと実BackendをChromium native WebAuthn/CTAP2へ接続する。
//! 固定expiry sessionを公開store APIで作るfixture境界と、14日実時間経過の未検証は同moduleに記す。

use std::error::Error;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{COOKIE, ORIGIN, SET_COOKIE};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use server_api::{
    ActorAuthMethod, AuthUserResponse, AuthenticatedUser, PasskeyAdditionOptionsRequest,
    PasskeyLoginCompleteRequest, PasskeyLoginOptionsResponse, PasskeyRegistrationCompleteRequest,
    PasskeyRegistrationOptionsResponse, WhoamiResponse,
};
#[path = "../src/resident_authenticator.rs"]
mod resident_model;
use resident_model::ResidentAuthenticator;
use yoi_e2e::WorkerE2eEnvironment;

#[path = "auth_passkeys/browser_process.rs"]
mod browser_process;
#[path = "auth_passkeys/browser_sessions.rs"]
mod browser_sessions;
#[path = "auth_passkeys/native_migration.rs"]
mod native_migration;

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
    authenticator: &mut ResidentAuthenticator,
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
    authenticator: &mut ResidentAuthenticator,
) -> Result<(AuthUserResponse, String)> {
    let options: PasskeyLoginOptionsResponse = decode(send(
        client
            .post(format!("{base}/api/auth/passkeys/login/options"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"browser_origin": BROWSER_ORIGIN})),
        "discoverable login options",
        artifacts,
    )?)?;
    let credential =
        authenticator.do_authentication(BROWSER_ORIGIN.parse()?, options.public_key)?;
    let result = session(send(
        client
            .post(format!("{base}/api/auth/passkeys/login/complete"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&PasskeyLoginCompleteRequest {
                challenge_id: options.challenge_id,
                credential,
            }),
        "verified login complete",
        artifacts,
    )?)?;
    assert_eq!(result.0.user.handle, handle);
    Ok(result)
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

fn browser_identity(
    client: &Client,
    base: &str,
    artifacts: &Path,
    cookie: &str,
    user: &AuthenticatedUser,
) -> Result {
    let whoami: WhoamiResponse = decode(send(
        client
            .get(format!("{base}/api/auth/whoami"))
            .header(COOKIE, cookie),
        "browser session rereads its own identity",
        artifacts,
    )?)?;
    let actor = whoami.actor.ok_or("browser session is anonymous")?;
    assert_eq!(actor.user(), *user);
    assert_eq!(actor.auth_method, ActorAuthMethod::BrowserSession);
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

// fixtureも現在の本人/認証bindingをwhoamiから取得する。期待値は対象を選ぶ権限ではない。
fn addition_expectation(
    request: RequestBuilder,
    artifacts: &Path,
) -> Result<PasskeyAdditionOptionsRequest> {
    let identity: WhoamiResponse = decode(send(
        request,
        "read addition owner and authentication reference",
        artifacts,
    )?)?;
    let actor = identity
        .actor
        .ok_or("addition fixture requires an authenticated actor")?;
    Ok(PasskeyAdditionOptionsRequest {
        expected_user_id: actor.user_id,
        expected_account_id: actor.account_id,
        expected_authentication_reference: identity
            .authentication_reference
            .ok_or("missing addition authentication reference")?,
        browser_origin: Some(BROWSER_ORIGIN.to_owned()),
    })
}

fn host_admin_login(
    environment: &WorkerE2eEnvironment,
    client: &Client,
    key: &mut ResidentAuthenticator,
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
    let addition_body = addition_expectation(
        client
            .get(format!("{base}/api/auth/whoami"))
            .bearer_auth(bearer),
        artifacts,
    )?;
    let options: PasskeyRegistrationOptionsResponse = decode(send(
        client
            .post(format!("{base}/api/auth/passkeys/addition/options"))
            .bearer_auth(bearer)
            .json(&addition_body),
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
    let mut admin_key = ResidentAuthenticator::new(true);
    let (admin, admin_cookie) = host_admin_login(&environment, &client, &mut admin_key)?;
    let admin_addition = addition_expectation(
        client
            .get(format!("{base}/api/auth/whoami"))
            .header(COOKIE, &admin_cookie),
        artifacts,
    )?;
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
                .json(&admin_addition),
            "unauthenticated addition options",
            artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "auth_required",
    )?;

    let mut original_key = ResidentAuthenticator::new(true);
    let (registered, registration_cookie, user_handle) = register(
        &client,
        base,
        artifacts,
        &environment.account_handle,
        &mut original_key,
    )?;
    // 登録sessionを失効させ、実assertionで得たlogin cookieだけで本人追加へ進む。
    logout(&client, base, artifacts, &registration_cookie)?;
    let (logged_in, cookie) = login(
        &client,
        base,
        artifacts,
        &environment.account_handle,
        &mut original_key,
    )?;
    assert_eq!(logged_in.user, registered.user);
    browser_identity(&client, base, artifacts, &cookie, &registered.user)?;
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
    let mut other_key = ResidentAuthenticator::new(true);
    let other_handle = format!("{}-other", environment.account_handle);
    let (other, _, other_user_handle) =
        register(&client, base, artifacts, &other_handle, &mut other_key)?;
    assert_ne!(other.user.account_id, registered.user.account_id);
    assert_ne!(other_user_handle, user_handle);
    access(&client, base, artifacts, &cookie, false)?;
    set_policy(&client, base, artifacts, &admin_cookie, false)?;
    // 以下の本人鍵追加と両方の鍵によるloginは、自己登録停止中にも必要。
    let options: PasskeyLoginOptionsResponse = decode(send(
        client
            .post(format!("{base}/api/auth/passkeys/login/options"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"browser_origin": BROWSER_ORIGIN})),
        "normal login has no account inventory",
        artifacts,
    )?)?;
    assert!(options.public_key.public_key.allow_credentials.is_empty());

    let addition_body = addition_expectation(
        client
            .get(format!("{base}/api/auth/whoami"))
            .header(COOKIE, &cookie),
        artifacts,
    )?;
    for (field, value) in [
        ("handle", &other.user.handle),
        ("user_id", &other.user.user_id),
        ("account_id", &other.user.account_id),
    ] {
        let mut body = serde_json::to_value(&addition_body)?;
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
            .json(&addition_body);
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
            .json(&addition_body),
        "authenticated self addition options",
        artifacts,
    )?)?;
    assert_eq!(
        serde_json::to_value(&options.public_key)?["publicKey"]["user"]["id"],
        user_handle,
        "same account must retain its WebAuthn user handle"
    );
    let mut added_key = ResidentAuthenticator::new(true);
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
    let (added, added_cookie) = session(send(
        client
            .post(format!("{base}/api/auth/passkeys/addition/complete"))
            .header(COOKIE, &cookie)
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&complete),
        "authenticated self addition complete",
        artifacts,
    )?)?;
    assert_eq!(added.user, registered.user);
    browser_identity(&client, base, artifacts, &added_cookie, &registered.user)?;
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

    // cookieを消すだけでなく、追加前後のsessionがServerでも失効したことを確かめる。
    logout(&client, base, artifacts, &cookie)?;
    logout(&client, base, artifacts, &added_cookie)?;
    for revoked_cookie in [&cookie, &added_cookie] {
        rejected(
            send(
                client
                    .post(format!("{base}/api/auth/passkeys/addition/options"))
                    .header(COOKIE, revoked_cookie)
                    .header(ORIGIN, BROWSER_ORIGIN)
                    .json(&addition_body),
                "logout prevents further self addition with the old cookie",
                artifacts,
            )?,
            StatusCode::FORBIDDEN,
            "auth_required",
        )?;
    }
    let (new_login, new_cookie) = login(
        &client,
        base,
        artifacts,
        &environment.account_handle,
        &mut added_key,
    )?;
    assert_eq!(new_login.user, registered.user);
    browser_identity(&client, base, artifacts, &new_cookie, &registered.user)?;
    let (old_login, old_cookie) = login(
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
    logout(&client, base, artifacts, &old_cookie)?;
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

    logout(&client, base, artifacts, &new_cookie)?;
    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    assert!(!report.fixture_root_exists_after);
    Ok(())
}

#[test]
fn workspace_catalog_zero_one_multiple_preserves_explicit_selection_and_actor_ownership() -> Result
{
    let mut environment = WorkerE2eEnvironment::new()?;
    eprintln!(
        "Workspace catalog E2E artifacts: {}",
        environment.artifacts_dir.display()
    );
    environment.start_empty_backend()?;
    assert!(environment.workspace_id.is_none());
    assert!(environment.runtime_pid().is_none());
    let base = environment
        .server_url
        .as_deref()
        .ok_or("missing server URL")?;
    let artifacts = &environment.artifacts_dir;
    let client = http_client()?;
    let empty: Value = decode(send(
        client.get(format!("{base}/api/workspaces")),
        "fresh empty Server exposes an empty anonymous catalog",
        artifacts,
    )?)?;
    assert_eq!(empty, json!([]));

    let request = json!({
        "operation_key": format!("catalog-e2e-{}-first", environment.runtime_id),
        "display_name": "Explicit first Workspace"
    });
    rejected(
        send(
            client
                .post(format!("{base}/api/workspaces"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&request),
            "anonymous create cannot bootstrap an owner",
            artifacts,
        )?,
        StatusCode::FORBIDDEN,
        "authenticated owner",
    )?;
    let mut admin_key = ResidentAuthenticator::new(true);
    let (_, admin_cookie) = host_admin_login(&environment, &client, &mut admin_key)?;
    set_policy(&client, base, artifacts, &admin_cookie, true)?;
    let mut key = ResidentAuthenticator::new(true);
    let (registered, registration_cookie, _) = register(
        &client,
        base,
        artifacts,
        &environment.account_handle,
        &mut key,
    )?;
    logout(&client, base, artifacts, &registration_cookie)?;
    let (logged_in, cookie) = login(
        &client,
        base,
        artifacts,
        &environment.account_handle,
        &mut key,
    )?;
    assert_eq!(logged_in.user, registered.user);
    browser_identity(&client, base, artifacts, &cookie, &registered.user)?;
    let empty: Value = decode(send(
        client
            .get(format!("{base}/api/workspaces"))
            .header(COOKIE, &cookie),
        "registration and explicit passkey login do not implicitly create a Workspace",
        artifacts,
    )?)?;
    assert_eq!(empty, json!([]));

    let mut expected = Vec::new();
    for (index, display_name) in ["Explicit first Workspace", "Explicit second Workspace"]
        .into_iter()
        .enumerate()
    {
        let mut request = request.clone();
        if index == 1 {
            request["operation_key"] =
                json!(format!("catalog-e2e-{}-second", environment.runtime_id));
            request["display_name"] = json!(display_name);
        }
        let response = send(
            client
                .post(format!("{base}/api/workspaces"))
                .header(COOKIE, &cookie)
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&request),
            "authenticated actor explicitly creates a Workspace without a Repository",
            artifacts,
        )?;
        assert_eq!(response.status(), StatusCode::CREATED);
        let created: Value = decode(response)?;
        assert_eq!(
            created["workspace"]["owner_account_id"],
            registered.user.account_id
        );
        assert_eq!(created["workspace"]["display_name"], display_name);
        assert_eq!(created["replayed"], false);
        assert!(created["repository"].is_null());
        let id = created["workspace"]["workspace_id"]
            .as_str()
            .ok_or("missing Workspace ID")?
            .to_owned();
        assert!(!expected.iter().any(|(previous, _)| previous == &id));
        expected.push((id, display_name));

        let listed: Value = decode(send(
            client
                .get(format!("{base}/api/workspaces"))
                .header(COOKIE, &cookie),
            "catalog rereads all explicitly created Workspaces without selecting one",
            artifacts,
        )?)?;
        let items = listed.as_array().ok_or("catalog is not an array")?;
        assert_eq!(items.len(), expected.len());
        // catalog順序に依存せず、返された各IDで独立したWorkspace contextを選択する。
        for (id, name) in &expected {
            let item = items
                .iter()
                .find(|item| item["workspace_id"] == *id)
                .ok_or("created Workspace is absent from catalog")?;
            assert_eq!(item["display_name"], *name);
            assert_eq!(item["owner_account_id"], registered.user.account_id);
            let selected: Value = decode(send(
                client
                    .get(format!("{base}/api/w/{id}/workspace"))
                    .header(COOKIE, &cookie),
                "explicit catalog ID selects the matching Workspace context",
                artifacts,
            )?)?;
            assert_eq!(selected["workspace_id"], *id);
            assert_eq!(selected["display_name"], *name);
            assert_eq!(selected["record_authority"], "server_db");
            assert_eq!(selected["permissions"]["manage_repositories"], true);
            let anonymous = send(
                client.get(format!("{base}/api/w/{id}/workspace")),
                "explicit Workspace ID alone does not authenticate an anonymous reader",
                artifacts,
            )?;
            assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
            let body = anonymous.text()?;
            assert!(!body.contains(id));
            assert!(!body.contains(name));
        }
        rejected(
            send(
                client.get(format!("{base}/api/workspaces")),
                "nonempty catalog is not exposed to anonymous visitors",
                artifacts,
            )?,
            StatusCode::UNAUTHORIZED,
            "authentication required",
        )?;
        let admin_catalog: Value = decode(send(
            client
                .get(format!("{base}/api/workspaces"))
                .header(COOKIE, &admin_cookie),
            "hosting administrator catalog does not include another actor's Workspaces",
            artifacts,
        )?)?;
        assert_eq!(admin_catalog, json!([]));
    }
    let unscoped = send(
        client
            .get(format!("{base}/api/workspace"))
            .header(COOKIE, &cookie),
        "multiple Workspaces cannot silently select the first unscoped API context",
        artifacts,
    )?;
    assert_eq!(unscoped.status(), StatusCode::NOT_FOUND);
    browser_identity(&client, base, artifacts, &cookie, &registered.user)?;
    logout(&client, base, artifacts, &cookie)?;
    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    assert!(!report.fixture_root_exists_after);
    Ok(())
}

fn device_login_pending(client: &Client, base: &str, artifacts: &Path, flow: &Value) -> Result {
    let pending: Value = decode(send(
        client
            .post(format!("{base}/api/auth/device-login/poll"))
            .json(&json!({"device_code": flow["device_code"]})),
        "device flow remains pending without explicit approval",
        artifacts,
    )?)?;
    assert_eq!(pending["status"], "pending");
    assert!(pending.get("access_token").is_none_or(Value::is_null));
    Ok(())
}

fn ordinary_device_login_requires_explicit_approval_and_does_not_grant_hosting_authority(
    client: &Client,
    base: &str,
    artifacts: &Path,
    user: &AuthUserResponse,
    key: &mut ResidentAuthenticator,
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
    device_login_pending(client, base, artifacts, &flow)?;
    // login APIの完了では認証だけが成立し、CLIへの権限付与はまだない。
    let (logged_in, cookie) = login(client, base, artifacts, &user.user.handle, key)?;
    assert_eq!(logged_in.user, user.user);
    browser_identity(client, base, artifacts, &cookie, &user.user)?;
    device_login_pending(client, base, artifacts, &flow)?;
    for origin in [None, Some("http://localhost:5174")] {
        let mut request = client
            .post(format!("{base}/api/auth/device-login/approve"))
            .header(COOKIE, &cookie)
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
    device_login_pending(client, base, artifacts, &flow)?;
    let approved: Value = decode(send(
        client
            .post(format!("{base}/api/auth/device-login/approve"))
            .header(COOKIE, &cookie)
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
    let whoami: WhoamiResponse = decode(send(
        client
            .get(format!("{base}/api/auth/whoami"))
            .bearer_auth(token),
        "explicit device approval issues the logged-in user's bearer",
        artifacts,
    )?)?;
    let actor = whoami.actor.ok_or("approved device bearer is anonymous")?;
    assert_eq!(actor.user(), user.user);
    assert_eq!(actor.auth_method, ActorAuthMethod::ApiToken);
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
    logout(client, base, artifacts, &cookie)?;
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

    let mut admin_key = ResidentAuthenticator::new(true);
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
    let mut ordinary_key = ResidentAuthenticator::new(true);
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
    let mut pending_key = ResidentAuthenticator::new(true);
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
    ordinary_device_login_requires_explicit_approval_and_does_not_grant_hosting_authority(
        &client,
        &base,
        &artifacts,
        &ordinary,
        &mut ordinary_key,
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

// code発行とその拒否はsuccess/rejectedの本文診断へ渡さない。
// 型にもDebugを付けず、失敗時にcode入りresponseをassertの値として出力しない。
#[derive(Deserialize)]
struct ActivationIssue {
    user: AuthenticatedUser,
    code: String,
    expires_at: String,
}

fn activation_success(response: Response) -> Result<Response> {
    let status = response.status();
    assert!(status.is_success(), "activation request failed: {status}");
    Ok(response)
}

#[track_caller]
fn activation_denied(response: Response) -> Result {
    let status = response.status();
    assert!(
        matches!(
            status,
            StatusCode::BAD_REQUEST
                | StatusCode::UNAUTHORIZED
                | StatusCode::FORBIDDEN
                | StatusCode::CONFLICT
                | StatusCode::UNPROCESSABLE_ENTITY
        ),
        "activation boundary returned {status}"
    );
    assert!(response.headers().get(SET_COOKIE).is_none());
    let text = response.text()?;
    // axumのJSON extractorはunknown-field拒否をtext/plainで返す。
    if status == StatusCode::UNPROCESSABLE_ENTITY {
        assert!(
            text.contains("unknown field"),
            "unexpected schema rejection"
        );
        return Ok(());
    }
    let body: Value = serde_json::from_str(&text)?;
    let object = body
        .as_object()
        .ok_or("rejection must be a structured API error")?;
    let keys: Vec<_> = object.keys().map(String::as_str).collect();
    let error_is_string = body
        .get("error")
        .and_then(Value::as_str)
        .is_some_and(|error| !error.is_empty());
    // cookie Origin middlewareは403のerror-only、typed handlerはmessage/diagnostics付き。
    // 想定外の形でも診断はstatusとJSON keysだけに限定し、errorやcodeの値を表示しない。
    let middleware_error = status == StatusCode::FORBIDDEN && object.len() == 1 && error_is_string;
    let typed_error = error_is_string
        && body
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|message| !message.is_empty())
        && body.get("diagnostics").is_some_and(Value::is_array);
    assert!(
        middleware_error || typed_error,
        "unexpected rejection shape: {status}, JSON keys={keys:?}"
    );
    for field in ["code", "user", "access_token", "token", "session"] {
        assert!(
            body.get(field).is_none_or(Value::is_null),
            "rejection leaked {field}"
        );
    }
    Ok(())
}

fn issue_activation(request: RequestBuilder, artifacts: &Path) -> Result<ActivationIssue> {
    let response = activation_success(send(
        request,
        "administrator issues activation code",
        artifacts,
    )?)?;
    assert!(response.headers().get(SET_COOKIE).is_none());
    let body: Value = response.json()?;
    for field in ["access_token", "token", "session"] {
        assert!(
            body.get(field).is_none_or(Value::is_null),
            "issuance granted {field}"
        );
    }
    let issue: ActivationIssue = serde_json::from_value(body)?;
    assert!(!issue.code.is_empty());
    assert!(!issue.expires_at.is_empty());
    Ok(issue)
}

fn create_pending(
    client: &Client,
    base: &str,
    artifacts: &Path,
    cookie: &str,
    handle: &str,
) -> Result<ActivationIssue> {
    let issue = issue_activation(
        client
            .post(format!("{base}/api/hosting/accounts"))
            .header(COOKIE, cookie)
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"handle": handle})),
        artifacts,
    )?;
    assert_eq!(issue.user.handle, handle);
    Ok(issue)
}

fn activation_options_request(
    client: &Client,
    base: &str,
    handle: &str,
    code: &str,
) -> RequestBuilder {
    client
        .post(format!("{base}/api/auth/activation/options"))
        .header(ORIGIN, BROWSER_ORIGIN)
        .json(&json!({"handle": handle, "code": code, "browser_origin": BROWSER_ORIGIN}))
}

fn activation_options(
    client: &Client,
    base: &str,
    artifacts: &Path,
    issue: &ActivationIssue,
) -> Result<PasskeyRegistrationOptionsResponse> {
    let response = activation_success(send(
        activation_options_request(client, base, &issue.user.handle, &issue.code),
        "code verification creates only limited registration options",
        artifacts,
    )?)?;
    assert!(response.headers().get(SET_COOKIE).is_none());
    let body: Value = response.json()?;
    for field in ["code", "user", "access_token", "token", "session"] {
        assert!(
            body.get(field).is_none_or(Value::is_null),
            "options granted {field}"
        );
    }
    Ok(serde_json::from_value(body)?)
}

fn activation_credential(
    options: PasskeyRegistrationOptionsResponse,
    key: &mut ResidentAuthenticator,
) -> Result<PasskeyRegistrationCompleteRequest> {
    Ok(PasskeyRegistrationCompleteRequest {
        challenge_id: options.challenge_id,
        credential: key.do_registration(BROWSER_ORIGIN.parse()?, options.public_key)?,
    })
}

fn activation_complete_request(
    client: &Client,
    base: &str,
    complete: &PasskeyRegistrationCompleteRequest,
) -> RequestBuilder {
    client
        .post(format!("{base}/api/auth/activation/complete"))
        .header(ORIGIN, BROWSER_ORIGIN)
        .json(complete)
}

// host CLIのFULL USER bearerを再利用せず、実sessionによるdevice承認からbearerを得る。
fn device_bearer(client: &Client, base: &str, artifacts: &Path, cookie: &str) -> Result<String> {
    let flow: Value = decode(send(
        client
            .post(format!("{base}/api/auth/device-login/start"))
            .json(&json!({"client_name": "isolated activation E2E"})),
        "start device login for live authority checks",
        artifacts,
    )?)?;
    let approved: Value = decode(send(
        client
            .post(format!("{base}/api/auth/device-login/approve"))
            .header(COOKIE, cookie)
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"user_code": flow["user_code"]})),
        "real passkey session approves device bearer",
        artifacts,
    )?)?;
    assert_eq!(approved["status"], "approved");
    let response = activation_success(send(
        client
            .post(format!("{base}/api/auth/device-login/poll"))
            .json(&json!({"device_code": flow["device_code"]})),
        "deliver device bearer without body diagnostics",
        artifacts,
    )?)?;
    let issued: Value = response.json()?;
    assert_eq!(issued["status"], "approved");
    Ok(issued["access_token"]
        .as_str()
        .ok_or("device bearer missing")?
        .to_owned())
}

#[test]
fn activation_with_disabled_self_registration_survives_reopen_and_enables_only_passkey_login()
-> Result {
    let mut environment = WorkerE2eEnvironment::new()?;
    eprintln!(
        "activation E2E artifacts: {}",
        environment.artifacts_dir.display()
    );
    environment.start_empty_backend()?;
    let client = http_client()?;
    let artifacts = environment.artifacts_dir.clone();
    let mut base = environment.server_url.clone().ok_or("missing server URL")?;
    let mut admin_key = ResidentAuthenticator::new(true);
    let (_, admin_cookie) = host_admin_login(&environment, &client, &mut admin_key)?;
    policy(&client, &base, &artifacts, false)?;
    let issue = create_pending(
        &client,
        &base,
        &artifacts,
        &admin_cookie,
        &environment.account_handle,
    )?;

    // optionsだけで取消し／reloadしてもcodeを消費せず、鍵なしaccountに通常権限を与えない。
    let _cancelled = activation_options(&client, &base, &artifacts, &issue)?;
    let _reloaded = activation_options(&client, &base, &artifacts, &issue)?;
    let whoami: Value = decode(send(
        client.get(format!("{base}/api/auth/whoami")),
        "activation options leave browser anonymous",
        &artifacts,
    )?)?;
    assert!(whoami["actor"].is_null());
    activation_denied(send(
        client
            .post(format!("{base}/api/auth/passkeys/login/options"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"handle": issue.user.handle, "browser_origin": BROWSER_ORIGIN})),
        "cancelled activation has no usable passkey",
        &artifacts,
    )?)?;
    environment.restart_backend()?;
    base = environment
        .server_url
        .clone()
        .ok_or("missing restarted server URL")?;
    policy(&client, &base, &artifacts, false)?;
    let options = activation_options(&client, &base, &artifacts, &issue)?;
    let mut key = ResidentAuthenticator::new(true);
    let complete = activation_credential(options, &mut key)?;
    let (activated, cookie) = session(activation_success(send(
        activation_complete_request(&client, &base, &complete),
        "verified activation after reopened pending state",
        &artifacts,
    )?)?)?;
    assert_eq!(activated.user, issue.user);
    access(&client, &base, &artifacts, &cookie, false)?;
    let workspaces: Value = decode(send(
        client
            .get(format!("{base}/api/workspaces"))
            .header(COOKIE, &cookie),
        "activation grants ordinary login without creating Workspace",
        &artifacts,
    )?)?;
    assert_eq!(workspaces, json!([]));
    activation_denied(send(
        activation_options_request(&client, &base, &issue.user.handle, &issue.code),
        "used activation code cannot log in again",
        &artifacts,
    )?)?;
    activation_denied(send(
        activation_complete_request(&client, &base, &complete),
        "completed activation challenge cannot be reused",
        &artifacts,
    )?)?;
    logout(&client, &base, &artifacts, &cookie)?;
    let (logged_in, cookie) = login(&client, &base, &artifacts, &issue.user.handle, &mut key)?;
    assert_eq!(logged_in.user, issue.user);
    access(&client, &base, &artifacts, &cookie, false)?;

    environment.restart_backend()?;
    base = environment
        .server_url
        .clone()
        .ok_or("missing reopened server URL")?;
    let (persisted, cookie) = login(&client, &base, &artifacts, &issue.user.handle, &mut key)?;
    assert_eq!(persisted.user, issue.user);
    access(&client, &base, &artifacts, &cookie, false)?;
    policy(&client, &base, &artifacts, false)?;
    activation_denied(send(
        activation_options_request(&client, &base, &issue.user.handle, &issue.code),
        "consumed code remains unusable after DB reopen",
        &artifacts,
    )?)?;
    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    assert!(!report.fixture_root_exists_after);
    Ok(())
}

#[test]
fn activation_issuance_requires_live_admin_and_cookie_origin_not_identity_claims() -> Result {
    let mut environment = WorkerE2eEnvironment::new()?;
    environment.start_empty_backend()?;
    let client = http_client()?;
    let artifacts = &environment.artifacts_dir;
    let base = environment
        .server_url
        .as_deref()
        .ok_or("missing server URL")?;
    let mut admin_key = ResidentAuthenticator::new(true);
    let (admin, admin_cookie) = host_admin_login(&environment, &client, &mut admin_key)?;
    let ordinary = create_pending(
        &client,
        base,
        artifacts,
        &admin_cookie,
        &environment.account_handle,
    )?;
    let mut ordinary_key = ResidentAuthenticator::new(true);
    let complete = activation_credential(
        activation_options(&client, base, artifacts, &ordinary)?,
        &mut ordinary_key,
    )?;
    let (_, ordinary_cookie) = session(activation_success(send(
        activation_complete_request(&client, base, &complete),
        "activate ordinary user for permission checks",
        artifacts,
    )?)?)?;
    let ordinary_bearer = device_bearer(&client, base, artifacts, &ordinary_cookie)?;
    let admin_bearer = device_bearer(&client, base, artifacts, &admin_cookie)?;
    let pending_handle = format!("{}-pending", environment.account_handle);
    let pending = create_pending(&client, base, artifacts, &admin_cookie, &pending_handle)?;

    for path in [
        "/api/hosting/accounts",
        "/api/hosting/accounts/activation-code",
    ] {
        for authorization in [None, Some(&ordinary_cookie), Some(&ordinary_bearer)] {
            let mut request = client
                .post(format!("{base}{path}"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .header("x-yoi-user-id", &admin.user.user_id)
                .header("x-yoi-account-id", &admin.user.account_id)
                .header("x-hosting-admin", "true")
                .json(&json!({"handle": pending_handle}));
            if let Some(authorization) = authorization {
                request = if authorization == &ordinary_cookie {
                    request.header(COOKIE, authorization)
                } else {
                    request.bearer_auth(authorization)
                };
            }
            activation_denied(send(
                request,
                "anonymous or ordinary identity spoof cannot issue code",
                artifacts,
            )?)?;
        }
        for origin in [None, Some("http://localhost:5174")] {
            let mut request = client
                .post(format!("{base}{path}"))
                .header(COOKIE, &admin_cookie)
                .json(&json!({"handle": pending_handle}));
            if let Some(origin) = origin {
                request = request.header(ORIGIN, origin);
            }
            activation_denied(send(
                request,
                "administrator cookie requires actual configured Origin",
                artifacts,
            )?)?;
        }
        activation_denied(send(
            client
                .post(format!("{base}{path}"))
                .bearer_auth(&admin_bearer)
                .header(ORIGIN, "http://localhost:5174")
                .json(&json!({"handle": pending_handle})),
            "bearer cannot bypass supplied untrusted browser Origin",
            artifacts,
        )?)?;
        for field in ["account_id", "user_id", "is_admin"] {
            let mut body = json!({"handle": pending_handle});
            body[field] = json!("forged");
            activation_denied(send(
                client
                    .post(format!("{base}{path}"))
                    .header(COOKIE, &admin_cookie)
                    .header(ORIGIN, BROWSER_ORIGIN)
                    .json(&body),
                "issuance body cannot select identity or grant administrator",
                artifacts,
            )?)?;
        }
    }
    // 拒否の後でも同じcodeで進める。拒否要求が暗黙に再発行していない。
    let _ = activation_options(&client, base, artifacts, &pending)?;
    let replacement = issue_activation(
        client
            .post(format!("{base}/api/hosting/accounts/activation-code"))
            .bearer_auth(&admin_bearer)
            .json(&json!({"handle": pending_handle})),
        artifacts,
    )?;
    assert_eq!(replacement.user, pending.user);
    activation_denied(send(
        activation_options_request(&client, base, &pending_handle, &pending.code),
        "legitimate bearer reissue invalidates old code",
        artifacts,
    )?)?;
    let _ = activation_options(&client, base, artifacts, &replacement)?;
    let bearer_created = issue_activation(
        client
            .post(format!("{base}/api/hosting/accounts"))
            .bearer_auth(&admin_bearer)
            .json(&json!({"handle": format!("{}-bearer", environment.account_handle)})),
        artifacts,
    )?;
    assert_ne!(bearer_created.user.account_id, pending.user.account_id);
    // 有効accountに対してcreateもcode再発行もcredential回復へ転用できない。
    for path in [
        "/api/hosting/accounts",
        "/api/hosting/accounts/activation-code",
    ] {
        activation_denied(send(
            client
                .post(format!("{base}{path}"))
                .header(COOKIE, &admin_cookie)
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"handle": ordinary.user.handle})),
            "administrator cannot replace active account or issue recovery code",
            artifacts,
        )?)?;
    }
    logout(&client, base, artifacts, &admin_cookie)?;
    for path in [
        "/api/hosting/accounts",
        "/api/hosting/accounts/activation-code",
    ] {
        activation_denied(send(
            client
                .post(format!("{base}{path}"))
                .header(COOKIE, &admin_cookie)
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"handle": pending_handle})),
            "revoked administrator cookie cannot issue code",
            artifacts,
        )?)?;
    }
    let (_, cookie) = login(
        &client,
        base,
        artifacts,
        &ordinary.user.handle,
        &mut ordinary_key,
    )?;
    access(&client, base, artifacts, &cookie, false)?;
    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    Ok(())
}

#[test]
fn activation_codes_challenges_and_unverified_state_cannot_cross_account_or_purpose() -> Result {
    let mut environment = WorkerE2eEnvironment::new()?;
    environment.start_empty_backend()?;
    let client = http_client()?;
    let base = environment
        .server_url
        .as_deref()
        .ok_or("missing server URL")?;
    let artifacts = &environment.artifacts_dir;
    let mut admin_key = ResidentAuthenticator::new(true);
    let (_, admin_cookie) = host_admin_login(&environment, &client, &mut admin_key)?;
    let issue = create_pending(
        &client,
        base,
        artifacts,
        &admin_cookie,
        &environment.account_handle,
    )?;
    let other = create_pending(
        &client,
        base,
        artifacts,
        &admin_cookie,
        &format!("{}-other", environment.account_handle),
    )?;
    for (handle, code) in [
        (issue.user.handle.as_str(), "invalid-code"),
        (other.user.handle.as_str(), issue.code.as_str()),
        ("unknown-activation-handle", issue.code.as_str()),
    ] {
        activation_denied(send(
            activation_options_request(&client, base, handle, code),
            "invalid code or account binding cannot start activation",
            artifacts,
        )?)?;
    }
    activation_denied(send(
        client
            .post(format!("{base}/api/auth/activation/options"))
            .header(ORIGIN, "https://example.invalid")
            .json(&json!({"handle": issue.user.handle, "code": issue.code,
                "browser_origin": "https://example.invalid"})),
        "untrusted browser origin cannot start activation",
        artifacts,
    )?)?;
    let options = activation_options(&client, base, artifacts, &issue)?;
    let challenge_id = options.challenge_id.clone();
    let mut key = ResidentAuthenticator::new(true);
    let complete = activation_credential(options, &mut key)?;

    // code／限定challenge IDのどちらも通常tokenではない。通常API、Workspace、device approvalに流用しない。
    let flow: Value = decode(send(
        client
            .post(format!("{base}/api/auth/device-login/start"))
            .json(&json!({"client_name": "pending activation must not approve"})),
        "start device flow before passkey verification",
        artifacts,
    )?)?;
    for bearer in [None, Some(issue.code.as_str()), Some(challenge_id.as_str())] {
        let mut request = client.get(format!("{base}/api/auth/whoami"));
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        let response = activation_success(send(
            request,
            "activation code and challenge leave whoami anonymous",
            artifacts,
        )?)?;
        assert!(response.headers().get(SET_COOKIE).is_none());
        let whoami: WhoamiResponse = response.json()?;
        assert!(whoami.actor.is_none());
        let requests = [
            client.get(format!("{base}/api/hosting/settings")),
            client.post(format!("{base}/api/workspaces")).header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"operation_key": format!("pending-{}", environment.runtime_id),
                    "display_name": "must not exist", "repository": {"repository_key": "fixture",
                        "uri": "https://example.invalid/activation.git", "default_ref": "develop"}})),
            client.post(format!("{base}/api/auth/device-login/approve")).header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"user_code": flow["user_code"]})),
            client.post(format!("{base}/api/auth/passkeys/addition/options")).header(ORIGIN, BROWSER_ORIGIN)
                .json(&json!({"browser_origin": BROWSER_ORIGIN, "expected_user_id": issue.user.user_id,
                    "expected_account_id": issue.user.account_id, "expected_authentication_reference": "unauthenticated-fixture-not-a-credential"})),
        ];
        for mut request in requests {
            if let Some(bearer) = bearer {
                request = request.bearer_auth(bearer);
            }
            activation_denied(send(
                request,
                "limited activation state grants no ordinary authority",
                artifacts,
            )?)?;
        }
    }
    device_login_pending(&client, base, artifacts, &flow)?;

    // completionはchallengeのtargetを使い、本文で別accountへ切り替えられない。
    for field in ["handle", "account_id", "user_id"] {
        let mut body = serde_json::to_value(&complete)?;
        body[field] = json!(&other.user.account_id);
        activation_denied(send(
            client
                .post(format!("{base}/api/auth/activation/complete"))
                .header(ORIGIN, BROWSER_ORIGIN)
                .json(&body),
            "activation completion rejects injected target identity",
            artifacts,
        )?)?;
    }
    for path in [
        "/api/auth/passkeys/registration/complete",
        "/api/auth/passkeys/addition/complete",
    ] {
        let mut request = client
            .post(format!("{base}{path}"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&complete);
        // additionでは実admin sessionを持っていてもactivation challengeの用途は変えられない。
        if path.contains("addition") {
            request = request.header(COOKIE, &admin_cookie);
        }
        activation_denied(send(
            request,
            "activation challenge cannot become public registration or self addition",
            artifacts,
        )?)?;
    }
    // 再発行はcodeだけでなく、それ以前に発行したchallengeも失効させる。
    let replacement = issue_activation(
        client
            .post(format!("{base}/api/hosting/accounts/activation-code"))
            .header(COOKIE, &admin_cookie)
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"handle": issue.user.handle})),
        artifacts,
    )?;
    assert_eq!(replacement.user, issue.user);
    activation_denied(send(
        activation_options_request(&client, base, &issue.user.handle, &issue.code),
        "old code cannot resume after reissue",
        artifacts,
    )?)?;
    activation_denied(send(
        activation_complete_request(&client, base, &complete),
        "old challenge cannot activate after reissue",
        artifacts,
    )?)?;
    // 別IDに対する誤ったcode入力が元のaccountを消費していない。
    let _ = activation_options(&client, base, artifacts, &other)?;
    let mut replacement_key = ResidentAuthenticator::new(true);
    let replacement_complete = activation_credential(
        activation_options(&client, base, artifacts, &replacement)?,
        &mut replacement_key,
    )?;
    let (user, cookie) = session(activation_success(send(
        activation_complete_request(&client, base, &replacement_complete),
        "replacement code activates only its bound account",
        artifacts,
    )?)?)?;
    assert_eq!(user.user, issue.user);
    access(&client, base, artifacts, &cookie, false)?;

    // 逆向きも拒否する。通常登録や本人鍵追加のchallengeでpending accountを有効化できない。
    set_policy(&client, base, artifacts, &admin_cookie, true)?;
    let public_handle = format!("{}-public", environment.account_handle);
    let public_options: PasskeyRegistrationOptionsResponse = decode(send(
        client
            .post(format!("{base}/api/auth/passkeys/registration/options"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"handle": public_handle, "browser_origin": BROWSER_ORIGIN})),
        "start distinct public registration ceremony",
        artifacts,
    )?)?;
    let mut public_key = ResidentAuthenticator::new(true);
    let public_complete = activation_credential(public_options, &mut public_key)?;
    activation_denied(send(
        activation_complete_request(&client, base, &public_complete),
        "public registration challenge cannot finish activation",
        artifacts,
    )?)?;
    let addition_options: PasskeyRegistrationOptionsResponse = decode(send(
        client
            .post(format!("{base}/api/auth/passkeys/addition/options"))
            .header(COOKIE, &cookie)
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&addition_expectation(
                client
                    .get(format!("{base}/api/auth/whoami"))
                    .header(COOKIE, &cookie),
                artifacts,
            )?),
        "start distinct authenticated self-addition ceremony",
        artifacts,
    )?)?;
    let mut addition_key = ResidentAuthenticator::new(true);
    let addition_complete = activation_credential(addition_options, &mut addition_key)?;
    activation_denied(send(
        activation_complete_request(&client, base, &addition_complete),
        "self-addition challenge cannot finish activation",
        artifacts,
    )?)?;
    set_policy(&client, base, artifacts, &admin_cookie, false)?;
    let (_, cookie) = login(
        &client,
        base,
        artifacts,
        &issue.user.handle,
        &mut replacement_key,
    )?;
    access(&client, base, artifacts, &cookie, false)?;
    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    Ok(())
}

#[test]
fn activation_bad_attestation_stays_pending_and_concurrent_ceremonies_store_only_one_key() -> Result
{
    let mut environment = WorkerE2eEnvironment::new()?;
    environment.start_empty_backend()?;
    let client = http_client()?;
    let artifacts = &environment.artifacts_dir;
    let base = environment
        .server_url
        .as_deref()
        .ok_or("missing server URL")?;
    let mut admin_key = ResidentAuthenticator::new(true);
    let (_, admin_cookie) = host_admin_login(&environment, &client, &mut admin_key)?;
    let issue = create_pending(
        &client,
        base,
        artifacts,
        &admin_cookie,
        &environment.account_handle,
    )?;
    let mut bad_key = ResidentAuthenticator::new(true);
    let bad_complete = activation_credential(
        activation_options(&client, base, artifacts, &issue)?,
        &mut bad_key,
    )?;
    let mut bad_attestation = serde_json::to_value(&bad_complete)?;
    // 外部JSONとして受理されるcredentialのclientDataだけを壊し、本番verifierで拒否させる。
    bad_attestation["credential"]["response"]["clientDataJSON"] = json!("e30");
    activation_denied(send(
        client
            .post(format!("{base}/api/auth/activation/complete"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&bad_attestation),
        "real WebAuthn verifier rejects malformed attestation",
        artifacts,
    )?)?;
    activation_denied(send(
        client
            .post(format!("{base}/api/auth/passkeys/login/options"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"handle": issue.user.handle, "browser_origin": BROWSER_ORIGIN})),
        "failed verifier did not activate or save a credential",
        artifacts,
    )?)?;
    let mut first_key = ResidentAuthenticator::new(true);
    let first = activation_credential(
        activation_options(&client, base, artifacts, &issue)?,
        &mut first_key,
    )?;
    let mut second_key = ResidentAuthenticator::new(true);
    let second = activation_credential(
        activation_options(&client, base, artifacts, &issue)?,
        &mut second_key,
    )?;
    let gate = std::sync::Barrier::new(3);
    let first_request = activation_complete_request(&client, base, &first);
    let second_request = activation_complete_request(&client, base, &second);
    // 独立した有効ceremonyを同時送信する。HTTP timeoutとRAII Server回収は既存harnessを使う。
    let (first_response, second_response) =
        std::thread::scope(|scope| -> Result<(Response, Response)> {
            let first_thread = scope.spawn(|| {
                gate.wait();
                first_request.send()
            });
            let second_thread = scope.spawn(|| {
                gate.wait();
                second_request.send()
            });
            gate.wait();
            Ok((
                first_thread
                    .join()
                    .map_err(|_| "first completion thread panicked")??,
                second_thread
                    .join()
                    .map_err(|_| "second completion thread panicked")??,
            ))
        })?;
    let first_won = first_response.status().is_success();
    let second_won = second_response.status().is_success();
    assert_ne!(
        first_won, second_won,
        "exactly one concurrent activation may succeed"
    );
    let (winner_response, loser_response, winner, winner_key, loser_key) = if first_won {
        (
            first_response,
            second_response,
            &first,
            &mut first_key,
            &mut second_key,
        )
    } else {
        (
            second_response,
            first_response,
            &second,
            &mut second_key,
            &mut first_key,
        )
    };
    activation_denied(loser_response)?;
    let (user, cookie) = session(activation_success(winner_response)?)?;
    assert_eq!(user.user, issue.user);
    access(&client, base, artifacts, &cookie, false)?;
    for complete in [&first, &second] {
        activation_denied(send(
            activation_complete_request(&client, base, complete),
            "neither successful nor losing ceremony can add a second activation key",
            artifacts,
        )?)?;
    }
    activation_denied(send(
        activation_options_request(&client, base, &issue.user.handle, &issue.code),
        "parallel activation consumed code once",
        artifacts,
    )?)?;
    let options: PasskeyLoginOptionsResponse = decode(send(
        client
            .post(format!("{base}/api/auth/passkeys/login/options"))
            .header(ORIGIN, BROWSER_ORIGIN)
            .json(&json!({"handle": issue.user.handle, "browser_origin": BROWSER_ORIGIN})),
        "reread only persisted credential after concurrent activation",
        artifacts,
    )?)?;
    let wire = serde_json::to_value(&options.public_key)?;
    let allowed = wire["publicKey"]["allowCredentials"]
        .as_array()
        .ok_or("login credentials missing")?;
    assert_eq!(
        allowed.len(),
        1,
        "failed verifier and losing ceremony must not store credentials"
    );
    assert_eq!(
        allowed[0]["id"],
        serde_json::to_value(winner)?["credential"]["id"]
    );
    assert!(
        loser_key
            .do_authentication(BROWSER_ORIGIN.parse()?, options.public_key)
            .is_err(),
        "losing authenticator must have no usable persisted credential"
    );
    let (logged_in, cookie) = login(&client, base, artifacts, &issue.user.handle, winner_key)?;
    assert_eq!(logged_in.user, issue.user);
    access(&client, base, artifacts, &cookie, false)?;
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
