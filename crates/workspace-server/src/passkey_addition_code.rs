//! Addition codes authorize only verified key insertion for an active owner.
//! The issuance response is the only secret disclosure: unknown issuance outcomes
//! require an owner status read and explicit reissue, never secret recovery.
//! Options and failed ceremonies leave the code usable. Credential insertion and
//! issuance deletion share one transaction; reissue/revoke fence pending ceremonies.

use super::*;
use ring::rand::{SecureRandom, SystemRandom};

pub(super) const MIGRATION_NAME: &str = "Owner-issued scoped passkey addition codes";

fn denied() -> Error {
    crate::auth::auth_error(
        "passkey_addition_code_invalid",
        "addition code is invalid, expired, locked or no longer current",
    )
}

pub(super) fn require_owner(
    conn: &Connection,
    user: &UserRecord,
    session_hash: &str,
    recent: bool,
) -> Result<()> {
    activation::require_active(conn, &user.user_id)?;
    let session: Option<(String, String)> = conn.query_row(
        "SELECT s.created_at,s.expires_at FROM browser_sessions s JOIN users u ON u.user_id=s.user_id WHERE s.token_hash=?1 AND u.user_id=?2 AND u.account_id=?3 AND s.revoked_at IS NULL",
        params![session_hash,user.user_id,user.account_id], |r| Ok((r.get(0)?,r.get(1)?)),
    ).optional()?;
    let (created_at, expires_at) = session.ok_or_else(|| {
        crate::auth::auth_error("auth_required", "a live owner browser session is required")
    })?;
    if crate::auth::is_expired(&expires_at) {
        return Err(crate::auth::auth_error(
            "auth_required",
            "browser session expired",
        ));
    }
    if recent
        && !chrono::DateTime::parse_from_rfc3339(&created_at)
            .ok()
            .is_some_and(|created| {
                let age = chrono::Utc::now().signed_duration_since(created);
                age >= chrono::Duration::zero() && age <= chrono::Duration::minutes(5)
            })
    {
        return Err(crate::auth::auth_error(
            "passkey_recent_authentication_required",
            "sign in again before issuing a passkey addition code",
        ));
    }
    Ok(())
}

pub(super) fn status(
    store: &SqliteWorkspaceStore,
    user: &UserRecord,
    session_hash: &str,
) -> Result<Option<server_api::PasskeyAdditionCodeStatus>> {
    store.with_conn(|conn| {
        require_owner(conn, user, session_hash, false)?;
        Ok(conn.query_row("SELECT issuance_id,target_origin,expires_at FROM passkey_addition_codes WHERE user_id=?1 AND failed_attempts<5", [&user.user_id], |r| Ok(server_api::PasskeyAdditionCodeStatus {
            issuance_id:r.get(0)?, target_origin:r.get(1)?, expires_at:r.get(2)?,
        })).optional()?.filter(|row| !crate::auth::is_expired(&row.expires_at)))
    })
}

pub(super) fn issue(
    store: &SqliteWorkspaceStore,
    user: &UserRecord,
    session_hash: &str,
    origin: &str,
    rp_id: &str,
) -> Result<server_api::PasskeyAdditionCodeResponse> {
    store.with_immediate_transaction(|tx| {
        require_owner(tx, user, session_hash, true)?;
        let mut bytes = [0u8;32];
        SystemRandom::new().fill(&mut bytes).map_err(|_| Error::Store("addition code entropy unavailable".into()))?;
        let code: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let issuance_id = crate::auth::new_id("passkey-addition");
        let expires_at = crate::auth::rfc3339_after(chrono::Duration::minutes(15));
        tx.execute("INSERT INTO passkey_addition_codes(user_id,issuance_id,code_hash,target_origin,rp_id,expires_at,failed_attempts) VALUES(?1,?2,?3,?4,?5,?6,0) ON CONFLICT(user_id) DO UPDATE SET issuance_id=excluded.issuance_id,code_hash=excluded.code_hash,target_origin=excluded.target_origin,rp_id=excluded.rp_id,expires_at=excluded.expires_at,failed_attempts=0",
            params![user.user_id,issuance_id,crate::auth::token_hash(&code),origin,rp_id,expires_at])?;
        Ok(server_api::PasskeyAdditionCodeResponse {
            user: server_api::AuthenticatedUser { user_id:user.user_id.clone(),account_id:user.account_id.clone(),handle:user.handle.clone(),display_name:user.display_name.clone() },
            issuance_id, code, target_origin:origin.into(), expires_at,
        })
    })
}

pub(super) fn revoke(
    store: &SqliteWorkspaceStore,
    user: &UserRecord,
    session_hash: &str,
    issuance_id: &str,
) -> Result<bool> {
    store.with_immediate_transaction(|tx| {
        require_owner(tx, user, session_hash, false)?;
        Ok(tx.execute(
            "DELETE FROM passkey_addition_codes WHERE user_id=?1 AND issuance_id=?2",
            params![user.user_id, issuance_id],
        )? == 1)
    })
}

pub(super) fn require_target(
    conn: &Connection,
    user: &UserRecord,
    issuance_id: &str,
    origin: &str,
    rp_id: &str,
) -> Result<()> {
    activation::require_active(conn, &user.user_id)?;
    let expiry: Option<String> = conn.query_row("SELECT c.expires_at FROM passkey_addition_codes c JOIN users u ON u.user_id=c.user_id WHERE u.user_id=?1 AND u.account_id=?2 AND c.issuance_id=?3 AND c.target_origin=?4 AND c.rp_id=?5 AND c.failed_attempts<5",
        params![user.user_id,user.account_id,issuance_id,origin,rp_id], |r| r.get(0)).optional()?;
    if !expiry.is_some_and(|expiry| !crate::auth::is_expired(&expiry)) {
        return Err(denied());
    }
    Ok(())
}

pub(super) fn verify(
    store: &SqliteWorkspaceStore,
    handle: &str,
    code: &str,
    origin: &str,
    rp_id: &str,
) -> Result<PasskeyRegistrationTarget> {
    let handle = crate::auth::normalize_handle(handle).map_err(|_| denied())?;
    // Rejected guesses commit their attempt counter, but return no authority.
    store.with_immediate_transaction(|tx| {
        let user = tx.query_row(user_select_sql("WHERE handle=?1").as_str(), [&handle], read_user_record).optional()?;
        let Some(user) = user else { return Ok(Err(denied())); };
        let row: Option<(String,String)> = tx.query_row("SELECT issuance_id,code_hash FROM passkey_addition_codes WHERE user_id=?1", [&user.user_id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((issuance_id,hash)) = row else { return Ok(Err(denied())); };
        if let Err(error) = require_target(tx,&user,&issuance_id,origin,rp_id) { return Ok(Err(error)); }
        if crate::auth::token_hash(code) != hash {
            tx.execute("UPDATE passkey_addition_codes SET failed_attempts=failed_attempts+1 WHERE user_id=?1", [&user.user_id])?;
            return Ok(Err(denied()));
        }
        Ok(Ok(PasskeyRegistrationTarget::AdditionCode { user,issuance_id,target_origin:origin.into(),rp_id:rp_id.into() }))
    })?
}

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 141 {
        return Err(Error::Store(
            "Addition code migration requires schema 141".into(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(include_str!("passkey_addition_code_schema.sql"))?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(142,?1)",
        [MIGRATION_NAME],
    )?;
    verify_workspace_resource_constraints(&tx)?;
    let violations: i64 =
        tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })?;
    if violations != 0 {
        return Err(Error::Store(
            "Addition code migration foreign-key violation".into(),
        ));
    }
    tx.commit()?;
    Ok(())
}
