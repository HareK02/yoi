//! Membership changes and replay receipts commit together. The collection observation fences
//! stale absence (including add/remove ABA); each grant has its own operation identity so a
//! stale removal cannot revoke a replacement grant. Replays never reapply historical grants.
use super::*;
use server_api::{
    WorkspaceMember, WorkspaceMemberAddRequest, WorkspaceMemberListQuery,
    WorkspaceMemberListResponse, WorkspaceMemberMutationResponse, WorkspaceMemberRemoveRequest,
};

pub(super) const MIGRATION_NAME: &str = "Owner-managed Workspace user membership";

pub(super) fn access(conn: &Connection, workspace: &str, account: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspaces w JOIN accounts a ON a.account_id=?2 AND a.kind='user' WHERE w.workspace_id=?1 AND w.state='active' AND (w.owner_account_id=?2 OR EXISTS(SELECT 1 FROM workspace_members m WHERE m.workspace_id=w.workspace_id AND m.account_id=?2)) AND NOT EXISTS(SELECT 1 FROM users u JOIN user_activations p ON p.user_id=u.user_id WHERE u.account_id=?2))",
        params![workspace, account], |r| r.get(0))?)
}

pub(crate) fn require_member(conn: &Connection, workspace: &str, account: &str) -> Result<()> {
    if !access(conn, workspace, account)? {
        return Err(Error::WorkspacePermissionDenied(
            "Workspace membership required".into(),
        ));
    }
    Ok(())
}

fn require_owner(conn: &Connection, workspace: &str, actor: &str) -> Result<()> {
    let allowed: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=?1 AND owner_account_id=?2 AND state='active')", params![workspace,actor], |r| r.get(0))?;
    if !allowed || !access(conn, workspace, actor)? {
        return Err(Error::WorkspacePermissionDenied(
            "Workspace membership management requires the active owner account".into(),
        ));
    }
    Ok(())
}

fn change_key(conn: &Connection, workspace: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT last_operation_key FROM workspace_membership_changes WHERE workspace_id=?1",
            [workspace],
            |r| r.get(0),
        )
        .optional()?)
}

fn text(value: &str, field: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 200
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(Error::InvalidInput(format!(
            "{field} must be nonempty bounded text without surrounding whitespace or controls"
        )));
    }
    Ok(())
}

pub(super) fn list(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    actor: &str,
    query: &WorkspaceMemberListQuery,
) -> Result<WorkspaceMemberListResponse> {
    let limit = query.limit.unwrap_or(100);
    if !(1..=100).contains(&limit) {
        return Err(Error::InvalidInput("member limit must be 1..100".into()));
    }
    if let Some(after) = &query.after_account_id {
        text(after, "after_account_id")?;
    }
    store.with_conn(|conn| {
        // A read transaction keeps the page and its collection observation consistent.
        let tx = conn.unchecked_transaction()?;
        require_owner(&tx, workspace, actor)?;
        let mut members = {
            let mut statement = tx.prepare("SELECT a.account_id,a.handle,a.display_name,a.account_id=w.owner_account_id,m.membership_key FROM workspaces w JOIN accounts a ON a.account_id=w.owner_account_id OR EXISTS(SELECT 1 FROM workspace_members x WHERE x.workspace_id=w.workspace_id AND x.account_id=a.account_id) LEFT JOIN workspace_members m ON m.workspace_id=w.workspace_id AND m.account_id=a.account_id WHERE w.workspace_id=?1 AND (?2 IS NULL OR a.account_id>?2) ORDER BY a.account_id LIMIT ?3")?;
            statement.query_map(params![workspace,query.after_account_id,limit+1], |r| Ok(WorkspaceMember { account_id:r.get(0)?, handle:r.get(1)?, display_name:r.get(2)?, is_owner:r.get(3)?, membership_key:r.get(4)? }))?.collect::<std::result::Result<Vec<_>,_>>()?
        };
        let next_after_account_id = if members.len() > limit as usize { members.truncate(limit as usize); members.last().map(|m| m.account_id.clone()) } else { None };
        let result = WorkspaceMemberListResponse { workspace_id:workspace.into(), membership_change_key:change_key(&tx,workspace)?, members, next_after_account_id };
        tx.commit()?;
        Ok(result)
    })
}

fn replay(
    conn: &Connection,
    workspace: &str,
    actor: &str,
    key: &str,
    request: &str,
) -> Result<Option<WorkspaceMemberMutationResponse>> {
    let saved: Option<(String,String,String)> = conn.query_row("SELECT actor_account_id,request_json,receipt_json FROM workspace_membership_operations WHERE workspace_id=?1 AND operation_key=?2", params![workspace,key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    match saved {
        None => Ok(None),
        Some((saved_actor, saved_request, receipt))
            if saved_actor == actor && saved_request == request =>
        {
            let mut receipt: WorkspaceMemberMutationResponse =
                serde_json::from_str(&receipt).map_err(|e| Error::Store(e.to_string()))?;
            receipt.replayed = true;
            Ok(Some(receipt))
        }
        Some(_) => Err(Error::RepositoryConflict(
            "membership operation key was used with different input".into(),
        )),
    }
}

fn save(
    conn: &Connection,
    actor: &str,
    key: &str,
    request: &str,
    receipt: &WorkspaceMemberMutationResponse,
) -> Result<()> {
    conn.execute("INSERT INTO workspace_membership_changes(workspace_id,last_operation_key) VALUES(?1,?2) ON CONFLICT(workspace_id) DO UPDATE SET last_operation_key=excluded.last_operation_key", params![receipt.workspace_id,key])?;
    conn.execute("INSERT INTO workspace_membership_operations(workspace_id,operation_key,actor_account_id,request_json,receipt_json,created_at) VALUES(?1,?2,?3,?4,?5,?6)", params![receipt.workspace_id,key,actor,request,serde_json::to_string(receipt).map_err(|e| Error::Store(e.to_string()))?,chrono::Utc::now().to_rfc3339()])?;
    Ok(())
}

pub(super) fn add(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    actor: &str,
    request: &WorkspaceMemberAddRequest,
) -> Result<WorkspaceMemberMutationResponse> {
    text(&request.operation_key, "operation_key")?;
    if let Some(key) = &request.expected_membership_change_key {
        text(key, "expected_membership_change_key")?;
    }
    let handle = crate::auth::normalize_handle(&request.account_handle)?;
    let input =
        serde_json::to_string(&("add", request)).map_err(|e| Error::Store(e.to_string()))?;
    store.with_immediate_transaction(|tx| {
        require_owner(tx,workspace,actor)?;
        if let Some(receipt) = replay(tx,workspace,actor,&request.operation_key,&input)? { return Ok(receipt); }
        if change_key(tx,workspace)? != request.expected_membership_change_key { return Err(Error::RepositoryConflict("Workspace membership observation changed; reread before adding".into())); }
        let account: Option<String> = tx.query_row("SELECT a.account_id FROM accounts a JOIN users u ON u.account_id=a.account_id WHERE a.kind='user' AND a.handle=?1 AND NOT EXISTS(SELECT 1 FROM user_activations p WHERE p.user_id=u.user_id)", [&handle], |r| r.get(0)).optional()?;
        let account = account.ok_or_else(|| Error::WorkspaceMemberAccountNotFound)?;
        let owner: String = tx.query_row("SELECT owner_account_id FROM workspaces WHERE workspace_id=?1", [workspace], |r| r.get(0))?;
        if account == owner || tx.query_row("SELECT EXISTS(SELECT 1 FROM workspace_members WHERE workspace_id=?1 AND account_id=?2)", params![workspace,account], |r| r.get::<_,bool>(0))? {
            return Err(Error::RepositoryConflict("Account is already a Workspace member".into()));
        }
        tx.execute("INSERT INTO workspace_members(workspace_id,account_id,membership_key) VALUES(?1,?2,?3)", params![workspace,account,request.operation_key])?;
        let receipt = WorkspaceMemberMutationResponse { workspace_id:workspace.into(),account_id:account,membership_key:Some(request.operation_key.clone()),membership_change_key:request.operation_key.clone(),present:true,replayed:false };
        save(tx,actor,&request.operation_key,&input,&receipt)?;
        Ok(receipt)
    })
}

pub(super) fn remove(
    store: &SqliteWorkspaceStore,
    workspace: &str,
    actor: &str,
    account: &str,
    request: &WorkspaceMemberRemoveRequest,
) -> Result<WorkspaceMemberMutationResponse> {
    text(&request.operation_key, "operation_key")?;
    text(&request.expected_membership_key, "expected_membership_key")?;
    text(account, "account_id")?;
    let input = serde_json::to_string(&("remove", account, request))
        .map_err(|e| Error::Store(e.to_string()))?;
    store.with_immediate_transaction(|tx| {
        require_owner(tx,workspace,actor)?;
        if let Some(receipt) = replay(tx,workspace,actor,&request.operation_key,&input)? { return Ok(receipt); }
        let owner: String = tx.query_row("SELECT owner_account_id FROM workspaces WHERE workspace_id=?1", [workspace], |r| r.get(0))?;
        if account == owner { return Err(Error::WorkspacePermissionDenied("The Workspace owner cannot be removed".into())); }
        if tx.execute("DELETE FROM workspace_members WHERE workspace_id=?1 AND account_id=?2 AND membership_key=?3", params![workspace,account,request.expected_membership_key])? != 1 {
            return Err(Error::RepositoryConflict("Workspace membership observation changed; reread before removing".into()));
        }
        let receipt = WorkspaceMemberMutationResponse { workspace_id:workspace.into(), account_id:account.into(),membership_key:None,membership_change_key:request.operation_key.clone(),present:false,replayed:false };
        save(tx,actor,&request.operation_key,&input,&receipt)?;
        Ok(receipt)
    })
}

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if current_schema_version(conn)? != 142 {
        return Err(Error::Store(
            "Workspace membership migration requires schema 142".into(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(include_str!("workspace_members_schema.sql"))?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(143,?1)",
        [MIGRATION_NAME],
    )?;
    verify_workspace_resource_constraints(&tx)?;
    let violations: i64 =
        tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })?;
    if violations != 0 {
        return Err(Error::Store(
            "Workspace membership migration foreign-key violation".into(),
        ));
    }
    tx.commit()?;
    Ok(())
}
