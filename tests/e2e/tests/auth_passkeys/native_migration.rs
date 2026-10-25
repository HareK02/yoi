//! 実Backend・本番SPA・Chromium native WebAuthn + CTAP2による旧鍵移行。
//! random user handle × resident=false/true × 同じ/別の認証器を検査する。
//! 旧鍵fixtureのuser.id/residency指定だけを変更し、実attestationを本番verifier/storeへ送る。
//! 現行登録を旧挙動と偽らず、SQL直書き・認証API/mock navigator置換はしない。
//! 起動/DB/HOME/XDG/port/回収はWorkerE2eEnvironment、browser profileはその一時領域が所有する。
//! DenoとPlaywright Chromium、最新production assets/binariesを呼出側が用意する。
//! `cargo test -p yoi-e2e --features e2e --test auth_passkeys native_ctap2 -- --ignored`
//! 暗黙buildを避ける実行時は既存providerの3つのbinary overrideを全て指定する。
//! DENO_DIR/PLAYWRIGHT_BROWSERS_PATHは事前取得済みtoolの供給元として明示する。
//! 物理鍵/実スマホ/provider選択は未確認。CI自動配線の代用にはしない。
use super::browser_process::BrowserProcess;
use super::*;
use std::net::TcpListener;
use std::process::Stdio;

#[test]
#[ignore = "requires production assets/binaries, cached Deno dependencies and Playwright Chromium"]
fn native_ctap2_legacy_random_handle_residency_exclusion_and_other_authenticator_migration()
-> Result {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let build = root.join("web/workspace/build");
    if !build.join("index.html").is_file() {
        return Err("native browser E2E requires an existing production Web build".into());
    }
    let mut environment = WorkerE2eEnvironment::new()?;
    let reservation = TcpListener::bind("127.0.0.1:0")?;
    let origin = format!("http://localhost:{}", reservation.local_addr()?.port());
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
    let response = http_client()?
        .put(format!("{base}/api/hosting/settings"))
        .bearer_auth(bearer)
        .json(&json!({"self_registration_enabled": true}))
        .send()?;
    if !response.status().is_success() {
        return Err("native fixture could not enable synthetic self-registration".into());
    }
    // Server public_urlと同一originを使う。API proxyだけでOrigin/Cookieを書き換えない。
    drop(reservation);
    let stderr =
        std::fs::File::create(environment.artifacts_dir.join("native-browser.stderr.log"))?;
    let mut command = environment.browser_command(Path::new("deno"))?;
    command
        .current_dir(&root)
        .args([
            "run",
            "--config",
            "tools/web-ux/deno.json",
            "--no-lock",
            "--cached-only",
            "--allow-env",
            "--allow-net=127.0.0.1,localhost",
            "--allow-read",
            "--allow-write",
            "--allow-run",
            "--allow-sys",
            "tests/e2e/browser/native_passkey_migration.ts",
            base,
            &origin,
            build.to_str().ok_or("non-UTF8 build path")?,
            &environment.account_handle,
            environment
                .artifacts_dir
                .to_str()
                .ok_or("non-UTF8 artifacts path")?,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    let process = BrowserProcess(Some(command.spawn()?));
    let status = process.wait_bounded(Duration::from_secs(180))?;
    if !status.success() {
        return Err(format!(
            "native CTAP2 matrix failed; sanitized diagnostics: {}",
            environment
                .artifacts_dir
                .join("native-browser.stderr.log")
                .display()
        )
        .into());
    }
    let report: Value = serde_json::from_slice(&std::fs::read(
        environment
            .artifacts_dir
            .join("native-migration-result.json"),
    )?)?;
    if report["status"] != "passed" || report["rows"].as_array().map(Vec::len) != Some(4) {
        return Err("native browser did not confirm all four matrix rows".into());
    }
    let cleanup = environment.cleanup()?;
    assert!(
        cleanup.cleanup_success,
        "owned native browser environment cleanup failed"
    );
    Ok(())
}
