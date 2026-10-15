//! 隔離した実yoi-serverのHTTP APIで、新規登録→明示handle login→本人の鍵追加→
//! owner-first Workspace作成を検証する。DB、認証、routerは本番実装を使い、
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

#[test]
fn fresh_account_adds_own_passkey_and_creates_owned_workspace() -> Result {
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
    let client = Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(5))
        .build()?;
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

    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    assert!(!report.fixture_root_exists_after);
    Ok(())
}
