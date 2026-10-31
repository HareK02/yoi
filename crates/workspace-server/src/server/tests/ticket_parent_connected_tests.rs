//! Normal Tool and native Inspect/Invoke -> signed production router -> Subject
//! authorization -> real SQLite. Reuses the shared workdirless Runtime fixture;
//! only external execution and one transport response are substituted.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
struct LosingResponse {
    inner: Arc<RouterClient>,
    lose: AtomicBool,
}
impl WorkspaceClient for LosingResponse {
    fn workspace_id(&self) -> Option<&str> {
        self.inner.workspace_id()
    }
    fn kind(&self) -> &str {
        "signed-parent-response-loss"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> std::result::Result<WorkspaceResponse, WorkspaceClientError> {
        let write =
            request.method == WorkspaceRequestMethod::Put && request.path.ends_with("/parent");
        let response = self.inner.execute(request)?;
        if write && response.status == 200 && self.lose.swap(false, Ordering::SeqCst) {
            return Err(WorkspaceClientError::Request(
                "injected response loss after commit".into(),
            ));
        }
        Ok(response)
    }
}
fn grant(operation: &str, id: &str) -> ScopedPermission {
    ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Ticket {
            workspace_id: TEST_WORKSPACE_ID.into(),
            ticket_id: id.into(),
        },
    }
}
fn nullable(value: Option<&str>) -> Value {
    match value {
        Some(value) => json!({"$case":"value","value":value}),
        None => json!({"$case":"null"}),
    }
}
struct Surface {
    native: bool,
    set: Arc<dyn Tool>,
    children: Arc<dyn Tool>,
    inspect: Option<Arc<dyn Tool>>,
}
impl Surface {
    async fn reference(&self, path: &str) -> std::result::Result<Value, agen::tool::ToolError> {
        let description = value(
            self.inspect
                .as_ref()
                .unwrap()
                .execute(
                    &json!({"path":path,"refresh":true}).to_string(),
                    ToolExecutionContext::direct(),
                )
                .await?,
        );
        Ok(description["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["reference"]["name"] == "yoi.ticket/item/v2")
            .expect("Ticket item interface")["reference"]
            .clone())
    }
    async fn invoke(
        &self,
        child: &str,
        expected: &ticket::TicketParentObservation,
        parent: Option<&str>,
        key: &str,
    ) -> std::result::Result<ToolOutput, agen::tool::ToolError> {
        let input = if self.native {
            let path = format!("/tickets/{child}");
            let parent_path = parent.map(|parent| format!("/tickets/{parent}"));
            json!({"path":path,"interface":self.reference(&path).await?,"operation":"parent_set","arguments":{
                "expected":{"parent_ticket_id":nullable(expected.parent_ticket_id.as_deref()),"last_operation_key":nullable(expected.last_operation_key.as_deref())},
                "parent":nullable(parent_path.as_deref()),"operation_key":key
            }})
        } else {
            json!({"ticket":child,"expected":expected,"parent":parent,"operation_key":key})
        };
        self.set
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
    }
    async fn children(
        &self,
        parent: &str,
    ) -> std::result::Result<ToolOutput, agen::tool::ToolError> {
        let input = if self.native {
            let path = format!("/tickets/{parent}");
            json!({"path":path,"interface":self.reference(&path).await?,"operation":"children_query","arguments":{"page":{"limit":1}}})
        } else {
            json!({"ticket":parent,"limit":1})
        };
        self.children
            .execute(&input.to_string(), ToolExecutionContext::direct())
            .await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tool_and_native_parent_mutations_share_metadata_only_authority_and_durable_replay() {
    for native in [false, true] {
        let f = Fixture::new().await;
        let backend = browser_ticket_backend(&f.api).unwrap();
        let a = backend.create(NewTicket::new("A private body")).unwrap().id;
        let b = backend.create(NewTicket::new("B private body")).unwrap().id;
        let child = backend
            .create(NewTicket::new("Child private body"))
            .unwrap()
            .id;
        let foreign = backend
            .create(NewTicket::new("Unauthorized Ticket"))
            .unwrap()
            .id;
        let key = |id: &str| backend.show(id.into()).unwrap().meta.resource_key.unwrap();
        let child_key = key(&child);
        let a_key = key(&a);
        let b_key = key(&b);
        let permissions = || {
            vec![
                grant("ticket.relation.read", &child),
                grant("ticket.relation.read", &a),
                grant("ticket.relation.read", &b),
                grant("ticket.relation.record", &child),
                grant("ticket.relation.record", &a),
                grant("ticket.relation.record", &b),
            ]
        };
        f.assign(permissions(), vec![]).await;
        assert_eq!(
            f.check(&f.source, &grant("ticket.read", &a)).await.0,
            StatusCode::FORBIDDEN
        );
        let client = Arc::new(LosingResponse {
            inner: f.client(&f.source),
            lose: AtomicBool::new(false),
        });
        let (mut host, installed) =
            ticket_worker_host(&f._dir.path().join("parent-host"), client.clone()).await;
        assert!(installed);
        if native {
            let mut mounts = worker::wip::WipMountRegistry::new();
            let route = mounts.allocate_namespace("ticket", "tickets").unwrap();
            worker::feature::builtin::ticket::mount_workspace_http_ticket_wip(
                &mut mounts,
                client.clone(),
                TicketFeatureAccess::workspace_authoring(),
                None,
                &route,
            )
            .unwrap();
            worker::wip::install_wip_mode_with_mounts(
                host.engine_mut(),
                None,
                "parent-contract".into(),
                mounts,
            )
            .unwrap();
        }
        let tools = host.engine().tool_server_handle();
        tools.flush_pending();
        let surface = Surface {
            native,
            set: tools
                .get_tool(if native { "Invoke" } else { "TicketParentSet" })
                .unwrap()
                .1,
            children: tools
                .get_tool(if native {
                    "Invoke"
                } else {
                    "TicketChildrenQuery"
                })
                .unwrap()
                .1,
            inspect: native.then(|| tools.get_tool("Inspect").unwrap().1),
        };
        let empty = Default::default();
        surface
            .invoke(&child_key, &empty, Some(&a_key), "attach")
            .await
            .unwrap();
        let first = backend
            .get_ticket_parent(child.clone().into())
            .unwrap()
            .observation;
        assert_eq!(first.parent_ticket_id.as_deref(), Some(a.as_str()));
        surface
            .invoke(&child_key, &first, Some(&a_key), "same-alias")
            .await
            .unwrap();
        assert_eq!(
            backend
                .get_ticket_parent(child.clone().into())
                .unwrap()
                .observation,
            first
        );
        assert!(
            surface
                .invoke(&child_key, &first, Some(&b_key), "reparent")
                .await
                .is_err()
        );
        assert_eq!(
            backend
                .get_ticket_parent(child.clone().into())
                .unwrap()
                .observation,
            first
        );
        let mut all = permissions();
        all.extend([
            grant("ticket.relation.remove", &child),
            grant("ticket.relation.remove", &a),
            grant("ticket.relation.remove", &b),
        ]);
        f.assign(all, vec![]).await;
        client.lose.store(true, Ordering::SeqCst);
        assert!(
            surface
                .invoke(&child_key, &first, Some(&b_key), "reparent")
                .await
                .is_err()
        );
        let moved = backend
            .get_ticket_parent(child.clone().into())
            .unwrap()
            .observation;
        assert_eq!(moved.parent_ticket_id.as_deref(), Some(b.as_str()));
        surface
            .invoke(&child_key, &first, Some(&b_key), "reparent")
            .await
            .unwrap();
        assert_eq!(
            backend
                .get_ticket_parent(child.clone().into())
                .unwrap()
                .observation,
            moved
        );
        surface.children(&b_key).await.unwrap();
        assert!(
            surface
                .invoke(&child_key, &first, None, "stale")
                .await
                .is_err()
        );
        surface
            .invoke(&child_key, &moved, None, "unlink")
            .await
            .unwrap();
        assert_eq!(
            backend
                .get_ticket_parent(child.clone().into())
                .unwrap()
                .observation
                .parent_ticket_id,
            None
        );
        let conn = rusqlite::Connection::open(&f.api.config.database_path).unwrap();
        let events: i64 = conn.query_row("SELECT COUNT(*) FROM typed_ticket_events WHERE workspace_id=?1 AND ticket_id=?2 AND kind='parent_changed'", rusqlite::params![TEST_WORKSPACE_ID,child], |row| row.get(0)).unwrap();
        assert_eq!(events, 3, "no-op and exact replay add no event");
        assert!(
            client
                .inner
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, _)| method == "GET")
                .all(|(_, path)| path.ends_with("/link-summary")
                    || path.contains("/children?limit=1")),
            "identity lookup must never fetch Ticket bodies or ancestors"
        );
        for path in [
            format!("/api/w/{TEST_WORKSPACE_ID}/tickets/{foreign}/link-summary"),
            format!("/api/w/another-workspace/tickets/{a}/link-summary"),
        ] {
            let (status, body) = response(
                &f.api,
                runtime_source_request(
                    &f.identity,
                    Some(&f.source.worker_id),
                    "GET",
                    &path,
                    vec![],
                ),
            )
            .await;
            assert!(!status.is_success(), "foreign/resource denial: {body}");
            assert!(body.get("title").is_none());
        }
    }
}
