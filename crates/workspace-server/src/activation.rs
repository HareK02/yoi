//! Activation codes authorize only a first passkey enrollment, never a normal actor.
//!
//! Options do not consume the code: cancellation/reload may resume with ID and code.
//! Each reissue replaces the issuance identity, invalidating every previous ceremony.
//! Credential insertion and removal of the pending row share the store's write transaction.
//! Only hashes are persisted; losing the one issuance response requires explicit reissue.

use super::*;
use ring::rand::{SecureRandom, SystemRandom};

/// Plaintext is returned only to the issuing administrator, never through user records.
#[derive(Clone)]
pub struct ActivationCodeIssue {
    pub user: UserRecord,
    pub code: String,
    pub expires_at: String,
}

impl std::fmt::Debug for ActivationCodeIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivationCodeIssue")
            .field("user", &self.user)
            .field("code", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

fn denied() -> Error {
    crate::auth::auth_error(
        "activation_code_invalid",
        "activation code is invalid, expired or locked",
    )
}

pub(super) fn pending(conn: &Connection, user_id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM user_activations WHERE user_id=?1)",
        [user_id],
        |r| r.get(0),
    )?)
}

pub(super) fn require_active(conn: &Connection, user_id: &str) -> Result<()> {
    if pending(conn, user_id)? {
        return Err(crate::auth::auth_error(
            "activation_required",
            "a first passkey must be registered before account use",
        ));
    }
    Ok(())
}

pub(super) fn require_active_account(conn: &Connection, account_id: &str) -> Result<()> {
    let pending: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM users u JOIN user_activations p ON p.user_id=u.user_id WHERE u.account_id=?1)", [account_id], |r| r.get(0))?;
    if pending {
        return Err(crate::auth::auth_error(
            "activation_required",
            "a pending account cannot own a Workspace",
        ));
    }
    Ok(())
}

fn require_admin(tx: &Connection, actor: &server_api::RequestActor) -> Result<()> {
    let allowed: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM hosting_administrators h JOIN users u ON u.user_id=h.user_id JOIN accounts a ON a.account_id=u.account_id WHERE u.user_id=?1 AND u.account_id=?2 AND a.kind='user' AND NOT EXISTS(SELECT 1 FROM user_activations p WHERE p.user_id=u.user_id))",
        params![actor.user_id, actor.account_id], |r| r.get(0),
    )?;
    if !allowed {
        return Err(Error::WorkspacePermissionDenied(
            "hosting_admin_required: a live hosting administrator is required".into(),
        ));
    }
    Ok(())
}

fn issue(tx: &Connection, user: UserRecord) -> Result<ActivationCodeIssue> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Store("activation code entropy unavailable".into()))?;
    let code: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let expires_at = crate::auth::rfc3339_after(chrono::Duration::minutes(15));
    let issuance_id = crate::auth::new_id("activation");
    tx.execute("INSERT INTO user_activations(user_id,issuance_id,code_hash,expires_at,failed_attempts) VALUES(?1,?2,?3,?4,0) ON CONFLICT(user_id) DO UPDATE SET issuance_id=excluded.issuance_id,code_hash=excluded.code_hash,expires_at=excluded.expires_at,failed_attempts=0",
        params![user.user_id, issuance_id, crate::auth::token_hash(&code), expires_at])?;
    Ok(ActivationCodeIssue {
        user,
        code,
        expires_at,
    })
}

pub(super) fn create(
    store: &SqliteWorkspaceStore,
    actor: &server_api::RequestActor,
    handle: &str,
) -> Result<ActivationCodeIssue> {
    let handle = crate::auth::normalize_handle(handle)?;
    store.with_immediate_transaction(|tx| {
        require_admin(tx, actor)?;
        let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM users WHERE handle=?1) OR EXISTS(SELECT 1 FROM accounts WHERE kind='user' AND handle=?1)", [&handle], |r| r.get(0))?;
        if occupied { return Err(crate::auth::auth_error("auth_handle_exists", "handle is already registered")); }
        let now = crate::auth::now_rfc3339();
        let user = UserRecord { user_id: crate::auth::new_id("user"), account_id: crate::auth::new_id("acct-user"), handle: handle.clone(), display_name: handle.clone(), created_at: now.clone(), updated_at: now.clone() };
        tx.execute("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES(?1,'user',?2,?2,?3,?3)", params![user.account_id, handle, now])?;
        tx.execute("INSERT INTO users(user_id,account_id,handle,display_name,created_at,updated_at) VALUES(?1,?2,?3,?3,?4,?4)", params![user.user_id, user.account_id, handle, now])?;
        issue(tx, user)
    })
}

pub(super) fn reissue(
    store: &SqliteWorkspaceStore,
    actor: &server_api::RequestActor,
    handle: &str,
) -> Result<ActivationCodeIssue> {
    let handle = crate::auth::normalize_handle(handle)?;
    store.with_immediate_transaction(|tx| {
        require_admin(tx, actor)?;
        let user = tx
            .query_row(
                user_select_sql("WHERE handle=?1").as_str(),
                [&handle],
                read_user_record,
            )
            .optional()?
            .ok_or_else(denied)?;
        if !pending(tx, &user.user_id)? {
            return Err(denied());
        }
        require_pending_target(tx, &user, None)?;
        issue(tx, user)
    })
}

/// A server-retained target is still checked against current account and issuance authority.
/// Reissue may replace an expired/locked issuance, but completion never may use one.
pub(super) fn require_pending_target(
    conn: &Connection,
    user: &UserRecord,
    issuance: Option<&str>,
) -> Result<()> {
    let valid: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM users u JOIN accounts a ON a.account_id=u.account_id JOIN user_activations p ON p.user_id=u.user_id WHERE u.user_id=?1 AND u.account_id=?2 AND u.handle=?3 AND a.kind='user' AND a.handle=u.handle AND NOT EXISTS(SELECT 1 FROM passkey_credentials c WHERE c.user_id=u.user_id))", params![user.user_id,user.account_id,user.handle], |r| r.get(0))?;
    if !valid {
        return Err(denied());
    }
    if let Some(issuance) = issuance {
        let expiry: Option<String> = conn.query_row("SELECT expires_at FROM user_activations WHERE user_id=?1 AND issuance_id=?2 AND failed_attempts<5", params![user.user_id,issuance], |r| r.get(0)).optional()?;
        if !expiry.is_some_and(|expiry| !crate::auth::is_expired(&expiry)) {
            return Err(denied());
        }
    }
    Ok(())
}

pub(super) fn verify(
    store: &SqliteWorkspaceStore,
    handle: &str,
    code: &str,
) -> Result<PasskeyRegistrationTarget> {
    let handle = crate::auth::normalize_handle(handle).map_err(|_| denied())?;
    // Inner result is deliberate: rejected attempts must commit before the public error.
    store.with_immediate_transaction(|tx| {
        let user = tx.query_row(user_select_sql("WHERE handle=?1").as_str(), [&handle], read_user_record).optional()?;
        let Some(user) = user else { return Ok(Err(denied())); };
        let row: Option<(String,String,String,i64)> = tx.query_row("SELECT issuance_id,code_hash,expires_at,failed_attempts FROM user_activations WHERE user_id=?1", [&user.user_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let Some((issuance_id,hash,expiry,attempts)) = row else { return Ok(Err(denied())); };
        if attempts >= 5 || crate::auth::is_expired(&expiry) { return Ok(Err(denied())); }
        if crate::auth::token_hash(code) != hash {
            tx.execute("UPDATE user_activations SET failed_attempts=failed_attempts+1 WHERE user_id=?1", [&user.user_id])?;
            return Ok(Err(denied()));
        }
        require_pending_target(tx, &user, Some(&issuance_id))?;
        Ok(Ok(PasskeyRegistrationTarget::Activation { user, issuance_id }))
    })?
}

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 99 {
        return Err(Error::Store(
            "Activation migration requires schema 99".into(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(include_str!("activation_schema.sql"))?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(100,?1)",
        ["Provisional users and scoped passkey activation"],
    )?;
    verify_workspace_resource_constraints(&tx)?;
    let violations: i64 =
        tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })?;
    if violations != 0 {
        return Err(Error::Store(
            "Activation migration foreign-key violation".into(),
        ));
    }
    tx.commit()?;
    Ok(())
}
