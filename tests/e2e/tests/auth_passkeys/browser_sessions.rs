//! 実Backend・本番static shell・Chromium cookie jarの接続を検証する。
//! navigator.credentialsだけを共有ResidentAuthenticatorの署名へ接続する。
//! 期限切れケースは実登録userへ公開store APIで固定expiry sessionを作り、
//! 本番cookie生成関数の短命primary/猶予markerをfixture HTTPで配信する。
//! SQL直書き・resolver/APIの置換・製品clockの追加はしない。
//! 実発行14日cookieの属性/DTOと短命fixtureの自然消失/復帰を分けて検証する。
//! OS/物理認証器と14日経過したServer時計は検証しない。
//! Deno/Playwright Chromiumと最新production buildが必要な明示選択suite:
//! `cargo test -p yoi-e2e --features e2e --test auth_passkeys real_backend_browser -- --ignored`
//! 通常auth_passkeysの成功/ignoreを、このbrowser接続の実行成功とは数えない。
use super::browser_process::BrowserProcess;
use super::*;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpListener;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Instant;
use yoi_workspace_server::auth::{
    SESSION_EXPIRY_GRACE_SECONDS, SessionCookiePolicy, mint_secret, new_id,
    session_marker_cookie_name, session_set_cookie, token_hash,
};
use yoi_workspace_server::store::{BrowserSessionRecord, ControlPlaneStore, SqliteWorkspaceStore};

// 戻り値はprivate IPC→fixture HTTPのSet-Cookieだけに使い、assert/debug/artifactへ出さない。
fn expired_session_fixture(environment: &WorkerE2eEnvironment) -> Result<Value> {
    // default環境や任意pathへ接続せず、このenvironmentが起動した既存DBだけを開く。
    let database = environment
        .server_data_dir
        .join("server.db")
        .canonicalize()?;
    if !database.starts_with(environment.root.canonicalize()?) {
        return Err("expired-session fixture DB is outside its environment".into());
    }
    let store = SqliteWorkspaceStore::open(database)?;
    let user = store
        .get_user_by_handle(&environment.account_handle)?
        .ok_or("browser registration must create the fixture user first")?;
    let token = mint_secret("yoi_sess");
    let hash = token_hash(&token);
    let now = chrono::Utc::now();
    // Server時計を動かさず、既に期限切れだが実猶予内のrecordを公開store APIで保存する。
    store.create_browser_session(&BrowserSessionRecord {
        session_id: new_id("session"),
        token_hash: hash.clone(),
        user_id: user.user_id,
        created_at: (now - chrono::Duration::days(14) - chrono::Duration::seconds(60)).to_rfc3339(),
        expires_at: (now - chrono::Duration::seconds(60)).to_rfc3339(),
        revoked_at: None,
    })?;
    let policy = SessionCookiePolicy {
        cookie_name: "yoi_workspace_session",
        path: "/",
        domain: None,
        secure: false,
    };
    let marker_name = session_marker_cookie_name(policy.cookie_name);
    Ok(json!({"set_cookies": [
        session_set_cookie(policy, &token, 2),
        session_set_cookie(SessionCookiePolicy {
            cookie_name: &marker_name,
            ..policy
        }, &hash, SESSION_EXPIRY_GRACE_SECONDS - 60),
    ]}))
}

#[test]
#[ignore = "requires production Web assets, Deno and Playwright Chromium"]
fn real_backend_browser_cookies_preserve_identity_expiry_return_url_and_explicit_logout() -> Result
{
    // Backend/process/DBの所有は既存harnessに限定する。Web buildは呼出元が用意する。
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let build = root.join("web/workspace/build");
    if !build.join("index.html").is_file() {
        return Err("build web/workspace production assets before selecting browser E2E".into());
    }
    let mut environment = WorkerE2eEnvironment::new()?;
    let reservation = TcpListener::bind("127.0.0.1:0")?;
    let port = reservation.local_addr()?.port();
    let origin = format!("http://localhost:{port}");
    std::fs::write(
        &environment.server_config,
        format!("[browser]\npublic_url = \"{origin}\"\n"),
    )?;
    environment.start_empty_backend()?;
    let admin =
        environment.setup_hosting_admin(&format!("{}-admin", environment.account_handle), true)?;
    let bearer = admin["access_token"]
        .as_str()
        .ok_or("missing setup token")?;
    let base = environment
        .server_url
        .as_deref()
        .ok_or("missing Backend URL")?;
    success(
        http_client()?
            .put(format!("{base}/api/hosting/settings"))
            .bearer_auth(bearer)
            .json(&json!({"self_registration_enabled": true}))
            .send()?,
    )?;
    drop(reservation);
    let stderr = std::fs::File::create(environment.artifacts_dir.join("browser.stderr.log"))?;
    let mut process = BrowserProcess(Some(
        environment
            .browser_command(Path::new("deno"))?
            .current_dir(&root)
            .args([
                "run",
                "--config",
                "tools/web-ux/deno.json",
                "--no-lock",
                "--allow-env",
                "--allow-net",
                "--allow-read",
                "--allow-write",
                "--allow-run",
                "--allow-sys",
                "tests/e2e/browser/auth_sessions.ts",
                base,
                &origin,
                build.to_str().ok_or("non-UTF8 build path")?,
                &environment.account_handle,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()?,
    ));
    let stdout = process
        .child()
        .stdout
        .take()
        .ok_or("missing browser stdout")?;
    let mut stdin = process
        .child()
        .stdin
        .take()
        .ok_or("missing browser stdin")?;
    let (send, receive) = mpsc::sync_channel(4);
    // bounded NDJSONは実WebAuthn options/署名とfixture cookieを運び、artifactには記録しない。
    let reader = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.by_ref().take(1024 * 1024).read_line(&mut line) {
                Ok(0) => break,
                Ok(_) if line.ends_with('\n') => {
                    if send.send(Ok(line)).is_err() {
                        break;
                    }
                }
                _ => {
                    let _ = send.send(Err("invalid or oversized browser IPC"));
                    break;
                }
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut authenticator = ResidentAuthenticator::new(true);
    let mut completed = false;
    loop {
        let message = match receive.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(message) => message.map_err(std::io::Error::other)?,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(_) => return Err("browser E2E timed out; see browser.stderr.log".into()),
        };
        let request: Value = serde_json::from_str(&message)?;
        let credential = match request["operation"].as_str() {
            Some("create") => serde_json::to_value(authenticator.do_registration(
                origin.parse()?,
                serde_json::from_value(request["options"].clone())?,
            )?)?,
            Some("get") => serde_json::to_value(authenticator.do_authentication(
                origin.parse()?,
                serde_json::from_value(request["options"].clone())?,
            )?)?,
            Some("expired-session") => expired_session_fixture(&environment)?,
            Some("done") => {
                completed = true;
                continue;
            }
            _ => return Err("unexpected browser IPC operation".into()),
        };
        serde_json::to_writer(&mut stdin, &credential)?;
        writeln!(stdin)?;
        stdin.flush()?;
    }
    reader.join().map_err(|_| "browser reader panicked")?;
    assert!(
        completed,
        "browser did not complete; see browser.stderr.log"
    );
    assert!(
        process.child().wait()?.success(),
        "browser E2E failed; see browser.stderr.log"
    );
    drop(process);
    let report = environment.cleanup()?;
    assert!(report.cleanup_success);
    Ok(())
}
