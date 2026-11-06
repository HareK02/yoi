//! Actual featureless yoi argv -> loopback yoi-server -> isolated persistent SQLite.
//! Uses the shared backend-only harness, stored bearer credentials and ordinary
//! editor files; no Runtime, model, scripted HTTP fixture, or test-support binary.
//! Run: cargo test -p yoi-e2e --features e2e --test configuration_cli -- --nocapture
//! Unknown-send recovery is covered separately by owner tests, not claimed here.

use std::{fs, path::Path, process::Output, time::Duration};

use serde_json::{Value, json};
use yoi_e2e::{WorkerE2eEnvironment, worker_binary_provider_info};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn assert_exit(output: Output, expected: i32) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(expected),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );
    format!("{stdout}{stderr}")
}

fn config(
    environment: &WorkerE2eEnvironment,
    binary: &Path,
    command: &str,
    directory: &Path,
    expected_exit: i32,
) -> Result<String> {
    let (backend, workspace) = environment.backend_target()?;
    let output = environment.run_cli(
        binary,
        &[
            "--backend",
            backend,
            "--workspace-id",
            workspace,
            "config",
            command,
            "--dir",
            directory.to_str().ok_or("non-UTF8 test path")?,
        ],
    )?;
    let result = assert_exit(output, expected_exit);
    eprintln!("config {command}: exit={expected_exit}\n{result}");
    Ok(result)
}

fn checkout_state(directory: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(
        directory.join(".yoi-config/state.json"),
    )?)?)
}

fn read_edit(directory: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(
        directory.join("edit.json"),
    )?)?)
}

#[test]
fn source_edit_push_repull_and_no_change_survive_backend_restart() -> Result {
    let binary = worker_binary_provider_info()?.binary("yoi")?;
    let mut environment = WorkerE2eEnvironment::new()?;
    eprintln!(
        "real product artifacts: {}",
        environment.artifacts_dir.display()
    );
    environment.start_backend()?;
    environment.install_backend_token(&environment.xdg_config_home)?;
    assert!(environment.server_data_dir.join("server.db").is_file());
    assert!(environment.runtime_pid().is_none());

    let first = environment.root.join("editor checkout");
    let initial_pull = config(&environment, &binary, "pull", &first, 0)?;
    assert!(initial_pull.contains("content_root="));
    let edit = read_edit(&first)?;
    assert!(edit["entrypoints"].is_array());
    assert!(edit["content_types"].is_object());
    let initial_state = checkout_state(&first)?;

    // Ordinary editor operation: an unreferenced text file is harmless to policy.
    let content = "Configuration CLI E2E: editable source persisted by the Server.\n";
    fs::write(first.join("source/editor-note.txt"), content)?;
    let source_before = fs::read(first.join("source/editor-note.txt"))?;
    let diff = config(&environment, &binary, "diff", &first, 0)?;
    assert!(diff.contains("create editor-note.txt"));
    assert!(diff.contains(content));
    let validate = config(&environment, &binary, "validate", &first, 0)?;
    assert!(validate.contains("validated expected_head="));
    let preview = config(&environment, &binary, "preview", &first, 0)?;
    assert!(preview.contains("editor-note.txt"));
    assert_eq!(
        checkout_state(&first)?,
        initial_state,
        "preview cannot adopt"
    );

    let push = config(&environment, &binary, "push", &first, 0)?;
    assert!(push.contains("source_stored=true"));
    assert!(push.contains("policy_committed=true"));
    assert_eq!(
        fs::read(first.join("source/editor-note.txt"))?,
        source_before
    );
    assert_eq!(read_edit(&first)?, edit);
    assert!(checkout_state(&first)?["pending"].is_null());

    // A re-pull must never replace the editor tree, even after confirmed adoption.
    let before_refused_pull = checkout_state(&first)?;
    let refusal = config(&environment, &binary, "pull", &first, 2)?;
    assert!(refusal.contains("NEW directory"));
    assert_eq!(checkout_state(&first)?, before_refused_pull);
    assert_eq!(
        fs::read(first.join("source/editor-note.txt"))?,
        source_before
    );

    let second = environment.root.join("repull");
    let second_pull = config(&environment, &binary, "pull", &second, 0)?;
    assert!(second_pull.contains("source_stored=true"));
    assert_eq!(
        fs::read_to_string(second.join("source/editor-note.txt"))?,
        content
    );
    let mut expected_edit = edit.clone();
    expected_edit["content_types"]["editor-note.txt"] = json!("text");
    assert_eq!(read_edit(&second)?, expected_edit);
    let stored = checkout_state(&second)?;
    assert_ne!(
        stored["base"]["history_head"],
        initial_state["base"]["history_head"]
    );
    let snapshot = stored["base"]["state"]["snapshot"].clone();
    assert_eq!(snapshot["entries"]["editor-note.txt"]["content"], content);
    assert_eq!(
        snapshot["entries"]["editor-note.txt"]["content_type"],
        "text"
    );

    // A no-change checkout must retain all omitted files, not delete them.
    let diff = config(&environment, &binary, "diff", &second, 0)?;
    assert!(!diff.lines().any(|line| {
        ["create ", "update ", "delete "]
            .iter()
            .any(|p| line.starts_with(p))
    }));
    let preview = config(&environment, &binary, "preview", &second, 0)?;
    assert!(preview.contains("changed_paths=[]"));
    config(&environment, &binary, "push", &second, 0)?;

    environment.restart_backend()?;
    // Backend may bind a new ephemeral port. Store credentials for its new origin.
    environment.install_backend_token(&environment.xdg_config_home)?;
    let restarted = environment.root.join("after restart");
    config(&environment, &binary, "pull", &restarted, 0)?;
    assert_eq!(
        checkout_state(&restarted)?["base"]["state"]["snapshot"],
        snapshot
    );
    assert_eq!(
        fs::read_to_string(restarted.join("source/editor-note.txt"))?,
        content
    );
    assert_eq!(read_edit(&restarted)?, expected_edit);
    assert!(environment.runtime_pid().is_none());
    assert!(environment.cleanup()?.cleanup_success);
    Ok(())
}

#[test]
fn authenticated_nonowner_cannot_read_preview_or_adopt_owner_workspace() -> Result {
    let binary = worker_binary_provider_info()?.binary("yoi")?;
    let mut environment = WorkerE2eEnvironment::new()?;
    eprintln!(
        "real product artifacts: {}",
        environment.artifacts_dir.display()
    );
    environment.start_backend()?;
    environment.install_backend_token(&environment.xdg_config_home)?;
    let owner_checkout = environment.root.join("owner checkout");
    config(&environment, &binary, "pull", &owner_checkout, 0)?;
    let original = checkout_state(&owner_checkout)?;
    fs::write(
        owner_checkout.join("source/denied.txt"),
        "must not be stored\n",
    )?;

    let handle = format!("nonowner-{}", environment.account_handle);
    // bootstrap-auth intentionally closes after the first Workspace. The public
    // host CLI can create another authenticated account at this stage. Its host
    // administration grant must NOT confer ownership of this Workspace.
    let nonowner = environment.setup_hosting_admin(&handle, true)?;
    let token = nonowner["access_token"]
        .as_str()
        .ok_or("missing nonowner token")?;
    let (backend, _) = environment.backend_target()?;
    // Prove these are valid credentials on this same Server, not anonymous denial.
    let response = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()?
        .get(format!("{backend}/api/hosting/access"))
        .bearer_auth(token)
        .send()?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.json::<Value>()?, json!({"is_admin": true}));

    let token_path = environment.xdg_config_home.join("yoi/backend-tokens.json");
    // Replace only this fixture's private credential store, preserving its 0600 mode.
    fs::write(
        &token_path,
        serde_json::to_vec(&json!({"tokens": {backend: {
            "token_type": "Bearer", "access_token": token
        }}}))?,
    )?;
    let denied_pull = environment.root.join("denied pull");
    assert!(
        config(&environment, &binary, "pull", &denied_pull, 5)?.contains("authorization refused")
    );
    assert!(!denied_pull.exists());
    for command in ["validate", "preview", "push"] {
        assert!(
            config(&environment, &binary, command, &owner_checkout, 5)?
                .contains("authorization refused")
        );
        assert_eq!(checkout_state(&owner_checkout)?, original);
    }

    // Re-read with the owner's stored credentials to prove denial had no effect.
    environment.install_backend_token(&environment.xdg_config_home)?;
    let verification = environment.root.join("owner verification");
    config(&environment, &binary, "pull", &verification, 0)?;
    assert_eq!(checkout_state(&verification)?["base"], original["base"]);
    assert!(!verification.join("source/denied.txt").exists());
    assert!(environment.cleanup()?.cleanup_success);
    Ok(())
}

#[test]
fn cli_help_and_input_errors_are_actionable_without_runtime() -> Result {
    let binary = worker_binary_provider_info()?.binary("yoi")?;
    let environment = WorkerE2eEnvironment::new()?;
    let help = assert_exit(environment.run_cli(&binary, &["config", "--help"])?, 0);
    for workflow in [
        "pull",
        "validate",
        "preview",
        "push",
        "NEW",
        "edit.json",
        "source/",
        "Configuration example",
        "Example workflow",
        "subject_id",
        "Exit codes",
    ] {
        assert!(
            help.contains(workflow),
            "missing workflow help for {workflow}: {help}"
        );
    }
    let error = assert_exit(environment.run_cli(&binary, &["config", "pull"])?, 1);
    assert!(help.contains("1 CLI argument/target-selection error"));
    assert!(error.contains("--dir"));
    assert!(environment.server_pid().is_none());
    assert!(environment.runtime_pid().is_none());
    assert!(environment.cleanup()?.cleanup_success);
    Ok(())
}
