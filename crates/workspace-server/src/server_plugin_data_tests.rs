//! Real Component/OwnerHost -> authenticated Remote or Embedded -> Ticket owner,
//! catalog, current Subject permissions and SQLite integration. The enclosing
//! suite supplies isolated DBs, signed Runtime proofs and RAII loopback servers;
//! only unrelated Worker execution is substituted. No live external service.
//! Run in the ordinary `yoi-workspace-server --lib plugin_host_tests` suite.
use super::*;
use server_api::{
    PluginDataAddress, PluginDataCell, PluginDataCollection, PluginDataExpectation,
    PluginDataMutation, PluginDataMutationLookup, PluginDataReceipt, PluginTicketAccess,
    PluginTicketCaseState,
};
use yoi_plugin_runtime::host::{Host, TicketDataOperation, errors, objects};

fn collection(version: &str) -> PluginDataCollection {
    PluginDataCollection {
        collection: "notes".into(),
        schema_id: "note".into(),
        schema_version: version.into(),
    }
}

fn address(key: &str) -> PluginDataAddress {
    PluginDataAddress {
        collection: collection("1"),
        key: key.into(),
    }
}

fn configured_grants(read: bool, write: bool) -> Value {
    json!({
        "ticket_read": read,
        "ticket_data_read": if read { vec![collection("1"), collection("2")] } else { vec![] },
        "ticket_data_write": if write { vec![collection("1"), collection("2")] } else { vec![] },
    })
}

fn data_manifest() -> String {
    use manifest::plugin::{PluginPackageManifest, PluginRequestedGrant};
    let mut manifest: PluginPackageManifest = toml::from_str(include_str!(
        "../../plugin-runtime/tests/ticket-guest/plugin.toml"
    ))
    .unwrap();
    // Both namespaces are explicit declarations, not a migration/implicit grant.
    let mut newer = manifest
        .data_collections
        .iter()
        .find(|c| c.collection == "notes" && c.schema_id == "note" && c.schema_version == "1")
        .unwrap()
        .clone();
    newer.schema_version = "2".into();
    manifest.data_collections.push(newer);
    manifest.requested_grants.extend([
        PluginRequestedGrant::TicketDataRead {
            collection: "notes".into(),
            schema_id: "note".into(),
            schema_version: "2".into(),
        },
        PluginRequestedGrant::TicketDataWrite {
            collection: "notes".into(),
            schema_id: "note".into(),
            schema_version: "2".into(),
        },
    ]);
    toml::to_string(&manifest).unwrap()
}

fn guest_component() -> &'static [u8] {
    // The parent generates this tracked Component from the actual shared
    // plugin-runtime/tests/ticket-guest Rust PDK source, using the encoding
    // recipe in plugin-runtime/tests/ticket_component.rs. No second guest.
    include_bytes!("../../plugin-pdk/tests/fixtures/ticket-tool-v2.component.wasm")
}

async fn data_fixture(read: bool, write: bool) -> Fixture {
    Fixture::with_package(
        None,
        data_manifest(),
        guest_component(),
        "ticket_test",
        Some(configured_grants(read, write)),
    )
    .await
}

fn create_ticket(f: &Fixture, body: &str) -> ticket::Ticket {
    create_ticket_with_slug(f, body, None)
}

fn create_ticket_with_slug(f: &Fixture, body: &str, slug: Option<&str>) -> ticket::Ticket {
    let backend = browser_ticket_backend(&f.api).unwrap();
    let mut input = ticket::NewTicket::new("Ordinary Ticket remains authoritative");
    input.slug = slug.map(str::to_owned);
    input.body = ticket::MarkdownText::new(body);
    let ticket = backend.create(input).unwrap();
    backend
        .add_event(
            ticket.id.clone().into(),
            ticket::NewTicketEvent::new(
                ticket::TicketEventKind::Comment,
                "private thread must not enter snapshot",
            ),
        )
        .unwrap();
    backend.show(ticket.id.into()).unwrap()
}

// Keep the real installation policy and change only concrete Ticket permissions.
// A cached resolve or receipt must not bypass these subsequent live changes.
fn ticket_policy(f: &Fixture, tickets: &[(&str, bool, bool)]) {
    let store = open_subjektiv_store(&f.api).unwrap();
    let previous = store.subject_permissions(&f.subject).unwrap().unwrap();
    let mut execution: Vec<_> = previous
        .execution
        .into_iter()
        .filter(|p| !matches!(p.resource, PermissionResource::Ticket { .. }))
        .collect();
    for &(ticket, read, write) in tickets {
        for (operation, granted) in [("ticket.read", read), ("ticket.extension.write", write)] {
            if granted {
                execution.push(ScopedPermission {
                    operation: operation.into(),
                    resource: PermissionResource::Ticket {
                        workspace_id: TEST_WORKSPACE_ID.into(),
                        ticket_id: ticket.into(),
                    },
                });
            }
        }
    }
    store
        .set_subject_permissions(
            &f.subject,
            &SubjectPermissionsUpdate {
                operation_id: Uuid::now_v7().to_string(),
                expected_change_id: Some(previous.change_id),
                execution,
                delegation: vec![],
            },
        )
        .unwrap();
}

async fn data_host(f: &Fixture, owner: Arc<dyn PluginHostOwner>, call: &str) -> Arc<OwnerHost> {
    host_for_plan(f, owner, &f.plan, &f.component, call).await
}

async fn host_for_plan(
    f: &Fixture,
    owner: Arc<dyn PluginHostOwner>,
    plan: &PluginPlanCreateReceipt,
    component: &CompiledTool,
    call: &str,
) -> Arc<OwnerHost> {
    use crate::plugin_plan::PluginGrant;
    let pin = &plan.plan.plugins()[0];
    let mut context = f.context();
    context.plan = plan.plan_id.clone();
    context.installation = pin.installation_id().into();
    context.artifact = component.identity().clone();
    context.subject = plan.plan.execution().principal_id.clone();
    context.grants = Grants::default();
    for grant in &pin.projection().grants {
        match grant {
            PluginGrant::Logging => context.grants.logging = true,
            PluginGrant::Https { slot } => {
                context.grants.connection_slots.insert(slot.clone());
            }
            PluginGrant::TicketRead => context.grants.ticket_read = true,
            PluginGrant::TicketDataRead {
                collection,
                schema_id,
                schema_version,
            } => {
                context.grants.ticket_data_read.insert((
                    collection.clone(),
                    schema_id.clone(),
                    schema_version.clone(),
                ));
            }
            PluginGrant::TicketDataWrite {
                collection,
                schema_id,
                schema_version,
            } => {
                context.grants.ticket_data_write.insert((
                    collection.clone(),
                    schema_id.clone(),
                    schema_version.clone(),
                ));
            }
        }
    }
    Arc::new(
        OwnerHost::begin(owner, context, f.consumer(call))
            .await
            .unwrap(),
    )
}

async fn resolve(
    owner: &Arc<dyn PluginHostOwner>,
    invocation: &str,
    ticket: &str,
    access: PluginTicketAccess,
) -> String {
    match owner
        .call(PluginHostRequest::ResolveTicket {
            invocation_id: invocation.into(),
            ticket_ref: ticket.into(),
            access,
        })
        .await
        .unwrap()
    {
        PluginHostResponse::TicketResolved { ticket_id } => ticket_id,
        other => panic!("expected resolved Ticket, got {other:?}"),
    }
}

fn get(invocation: &str, ticket: &str, address: PluginDataAddress) -> PluginHostRequest {
    PluginHostRequest::DataGet {
        invocation_id: invocation.into(),
        ticket_id: ticket.into(),
        address,
    }
}

fn put(
    invocation: &str,
    ticket: &str,
    address: PluginDataAddress,
    key: &str,
    expected: PluginDataExpectation,
    text: &str,
) -> PluginHostRequest {
    PluginHostRequest::DataMutate {
        invocation_id: invocation.into(),
        ticket_id: ticket.into(),
        address,
        operation_key: key.into(),
        mutation: PluginDataMutation::Put {
            expected,
            value_json: json!(text).to_string(),
        },
    }
}

fn lookup(
    invocation: &str,
    ticket: &str,
    address: PluginDataAddress,
    key: &str,
) -> PluginHostRequest {
    PluginHostRequest::DataLookupMutation {
        invocation_id: invocation.into(),
        ticket_id: ticket.into(),
        address,
        operation_key: key.into(),
    }
}

fn receipt(response: PluginHostResponse) -> PluginDataReceipt {
    match response {
        PluginHostResponse::DataMutated { receipt } => receipt,
        other => panic!("expected mutation receipt, got {other:?}"),
    }
}

fn assert_ticket_unchanged(f: &Fixture, before: &ticket::Ticket) {
    let after = browser_ticket_backend(&f.api)
        .unwrap()
        .show(before.meta.id.clone().into())
        .unwrap();
    assert_eq!(after.meta.title, before.meta.title);
    assert_eq!(after.document.body, before.document.body);
    assert_eq!(after.meta.workflow_state, before.meta.workflow_state);
    assert_eq!(
        ticket::ticket_content_digest(&after),
        ticket::ticket_content_digest(before)
    );
    assert_eq!(
        after.events, before.events,
        "extension data must not add ordinary thread events"
    );
    assert_eq!(after.case_history, before.case_history);
    assert_eq!(after.current_closure, before.current_closure);
    assert_eq!(after.closure_operation_key, before.closure_operation_key);
}

#[tokio::test]
async fn component_ticket_data_remote_and_embedded_preserve_ordinary_ticket() {
    for use_remote in [false, true] {
        let f = data_fixture(true, true).await;
        // The shared real guest deliberately resolves this fixed selector.
        // A real Ticket slug supplies it; the ordinary owner resolver still
        // verifies current Subject permissions and returns its canonical ID.
        let before = create_ticket_with_slug(
            &f,
            "Ordinary **body** is not extension data.",
            Some("T-795"),
        );
        let before = if use_remote {
            browser_ticket_backend(&f.api)
                .unwrap()
                .update_case(
                    &before.meta.id,
                    ticket::TicketCaseUpdate {
                        expected_content_digest: ticket::ticket_content_digest(&before),
                        expected_state: ticket::TicketState::Open,
                        expected_closure_operation_key: before.closure_operation_key.clone(),
                        operation_key: "fixture-close".into(),
                        action: ticket::TicketCaseAction::Close {
                            reason: ticket::TicketClosureReason::Completed,
                        },
                        explanation:
                            "Retained ordinary case judgment must not change with extension data"
                                .into(),
                        references: vec![],
                        author: Some("fixture-owner".into()),
                    },
                )
                .unwrap()
        } else {
            before
        };
        ticket_policy(&f, &[(&before.meta.id, true, true)]);
        let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
        let owner = if use_remote {
            remote
        } else {
            f.owner(None).await
        };
        let host = data_host(&f, owner.clone(), "component-data").await;
        let invocation = host.context().invocation_id.clone();
        let result = f
            .executor
            .invoke(
                &f.component,
                host.context().clone(),
                host,
                f.tool,
                r#""exercise""#,
                "{}",
            )
            .await
            .unwrap();
        assert_eq!(result.output.summary, before.meta.title);
        assert_eq!(result.output.structured_json.as_deref(), Some("null"));
        // The shared guest puts/reads/lists/looks up/deletes/recreates and then
        // replays its original put. Check persisted effects with another real
        // adapter, not only guest assertions or an invented output envelope.
        let embedded = f.owner(None).await;
        let written = match embedded
            .call(lookup(&invocation, &before.meta.id, address("main"), "put"))
            .await
            .unwrap()
        {
            PluginHostResponse::DataMutationLookup {
                outcome: PluginDataMutationLookup::Committed(receipt),
            } => receipt,
            other => panic!("guest put receipt expected: {other:?}"),
        };
        let recreated = match embedded
            .call(lookup(
                &invocation,
                &before.meta.id,
                address("main"),
                "recreate",
            ))
            .await
            .unwrap()
        {
            PluginHostResponse::DataMutationLookup {
                outcome: PluginDataMutationLookup::Committed(receipt),
            } => receipt,
            other => panic!("guest recreate receipt expected: {other:?}"),
        };
        assert_ne!(written.mutation_id, recreated.mutation_id);
        assert_eq!(
            embedded
                .call(get(&invocation, &before.meta.id, address("main")))
                .await
                .unwrap(),
            PluginHostResponse::DataCell {
                cell: PluginDataCell::Present {
                    value_json: r#""note""#.into(),
                    last_write: recreated.mutation_id,
                }
            }
        );
        assert_ticket_unchanged(&f, &before);
        drop(server);
        f.shutdown().await;
    }
}

#[tokio::test]
async fn adapter_data_replay_precedes_cas_and_delete_tombstone_prevents_aba() {
    let f = data_fixture(true, true).await;
    let before = create_ticket(&f, "CAS does not edit Ticket text");
    ticket_policy(&f, &[(&before.meta.id, true, true)]);
    let embedded = f.owner(None).await;
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let host = data_host(&f, remote.clone(), "cas-replay").await;
    let id = &host.context().invocation_id;
    let ticket = resolve(
        &remote,
        id,
        &before.meta.id,
        PluginTicketAccess::ExtensionWrite,
    )
    .await;
    assert_eq!(
        remote
            .call(get(id, &ticket, address("cell")))
            .await
            .unwrap(),
        PluginHostResponse::DataCell {
            cell: PluginDataCell::NeverWritten
        }
    );
    let first_request = put(
        id,
        &ticket,
        address("cell"),
        "create",
        PluginDataExpectation::NeverWritten,
        "first",
    );
    let first = receipt(remote.call(first_request.clone()).await.unwrap());
    let second = receipt(
        embedded
            .call(put(
                id,
                &ticket,
                address("cell"),
                "replace",
                PluginDataExpectation::LastWrite(first.mutation_id.clone()),
                "second",
            ))
            .await
            .unwrap(),
    );
    assert_eq!(
        receipt(embedded.call(first_request).await.unwrap()),
        first,
        "replay is not a fresh CAS"
    );
    assert_eq!(
        remote
            .call(put(
                id,
                &ticket,
                address("cell"),
                "stale",
                PluginDataExpectation::LastWrite(first.mutation_id.clone()),
                "third"
            ))
            .await
            .unwrap_err()
            .code,
        PluginHostErrorCode::Conflict
    );
    assert_eq!(
        remote
            .call(put(
                id,
                &ticket,
                address("cell"),
                "create",
                PluginDataExpectation::NeverWritten,
                "changed input"
            ))
            .await
            .unwrap_err()
            .code,
        PluginHostErrorCode::Conflict
    );
    let deleted = receipt(
        remote
            .call(PluginHostRequest::DataMutate {
                invocation_id: id.clone(),
                ticket_id: ticket.clone(),
                address: address("cell"),
                operation_key: "delete".into(),
                mutation: PluginDataMutation::Delete {
                    expected_last_write: second.mutation_id,
                },
            })
            .await
            .unwrap(),
    );
    assert_eq!(
        embedded
            .call(get(id, &ticket, address("cell")))
            .await
            .unwrap(),
        PluginHostResponse::DataCell {
            cell: PluginDataCell::Deleted {
                last_delete: deleted.mutation_id.clone()
            }
        }
    );
    assert_eq!(
        remote
            .call(put(
                id,
                &ticket,
                address("cell"),
                "never-again",
                PluginDataExpectation::NeverWritten,
                "recreated"
            ))
            .await
            .unwrap_err()
            .code,
        PluginHostErrorCode::Conflict
    );
    let recreated = receipt(
        embedded
            .call(put(
                id,
                &ticket,
                address("cell"),
                "recreate",
                PluginDataExpectation::LastWrite(deleted.mutation_id.clone()),
                "recreated",
            ))
            .await
            .unwrap(),
    );
    assert_ne!(recreated.mutation_id, deleted.mutation_id);
    assert_eq!(
        remote
            .call(lookup(id, &ticket, address("cell"), "create"))
            .await
            .unwrap(),
        PluginHostResponse::DataMutationLookup {
            outcome: PluginDataMutationLookup::Committed(first)
        }
    );
    assert!(
        matches!(remote.call(get(id, &ticket, address("cell"))).await.unwrap(),
        PluginHostResponse::DataCell { cell: PluginDataCell::Present { last_write, .. } } if last_write == recreated.mutation_id)
    );
    assert_ticket_unchanged(&f, &before);
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn adapter_data_object_schema_namespaces_and_list_cursors_cannot_transfer() {
    let f = data_fixture(true, true).await;
    let a = create_ticket(&f, "Object A");
    let b = create_ticket(&f, "Object B");
    ticket_policy(&f, &[(&a.meta.id, true, true), (&b.meta.id, true, true)]);
    let embedded = f.owner(None).await;
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let host = data_host(&f, embedded.clone(), "namespace-cursors").await;
    let id = &host.context().invocation_id;
    for key in ["a", "b"] {
        remote
            .call(put(
                id,
                &a.meta.id,
                address(key),
                key,
                PluginDataExpectation::NeverWritten,
                key,
            ))
            .await
            .unwrap();
    }
    assert_eq!(
        remote
            .call(get(id, &b.meta.id, address("a")))
            .await
            .unwrap(),
        PluginHostResponse::DataCell {
            cell: PluginDataCell::NeverWritten
        }
    );
    let mut newer = address("a");
    newer.collection.schema_version = "2".into();
    assert_eq!(
        embedded
            .call(get(id, &a.meta.id, newer.clone()))
            .await
            .unwrap(),
        PluginHostResponse::DataCell {
            cell: PluginDataCell::NeverWritten
        }
    );
    remote
        .call(put(
            id,
            &a.meta.id,
            newer,
            "new-schema",
            PluginDataExpectation::NeverWritten,
            "version two",
        ))
        .await
        .unwrap();
    assert!(
        matches!(embedded.call(get(id, &a.meta.id, address("a"))).await.unwrap(),
        PluginHostResponse::DataCell { cell: PluginDataCell::Present { value_json, .. } } if value_json == r#""a""#)
    );
    let request = PluginHostRequest::DataList {
        invocation_id: id.clone(),
        ticket_id: a.meta.id.clone(),
        collection: collection("1"),
        cursor: None,
        limit: 1,
    };
    let PluginHostResponse::DataListed { page } = remote.call(request.clone()).await.unwrap()
    else {
        panic!("list expected")
    };
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].key, "a");
    let cursor = page.next_cursor.unwrap();
    let mut continuation = request;
    if let PluginHostRequest::DataList { cursor: value, .. } = &mut continuation {
        *value = Some(cursor);
    }
    let PluginHostResponse::DataListed { page } =
        embedded.call(continuation.clone()).await.unwrap()
    else {
        panic!("list expected")
    };
    assert_eq!(page.entries[0].key, "b");
    for change_schema in [false, true] {
        let mut foreign = continuation.clone();
        if let PluginHostRequest::DataList {
            ticket_id,
            collection,
            ..
        } = &mut foreign
        {
            if change_schema {
                collection.schema_version = "2".into();
            } else {
                *ticket_id = b.meta.id.clone();
            }
        }
        assert_eq!(
            remote.call(foreign).await.unwrap_err().code,
            PluginHostErrorCode::InvalidInput
        );
    }
    let mut undeclared = address("a");
    undeclared.collection.schema_id = "foreign.schema".into();
    assert_denied(embedded.call(get(id, &a.meta.id, undeclared)).await);
    assert_eq!(
        remote
            .call(lookup(id, &b.meta.id, address("a"), "a"))
            .await
            .unwrap_err()
            .code,
        PluginHostErrorCode::Conflict,
        "a receipt is not transferable to another object"
    );
    assert_ticket_unchanged(&f, &a);
    assert_ticket_unchanged(&f, &b);
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn resolved_ticket_and_saved_data_receipt_recheck_live_permissions_and_revocation() {
    let f = data_fixture(true, true).await;
    let before = create_ticket(&f, "Live permission subject");
    ticket_policy(&f, &[(&before.meta.id, true, true)]);
    let embedded = f.owner(None).await;
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let host = data_host(&f, embedded.clone(), "live-data-policy").await;
    let id = &host.context().invocation_id;
    let binding = host
        .resolve_ticket(
            host.context(),
            &before.meta.id,
            objects::Access::ExtensionWrite,
        )
        .await
        .unwrap();
    let request = put(
        id,
        &before.meta.id,
        address("retained"),
        "write",
        PluginDataExpectation::NeverWritten,
        "retained",
    );
    remote.call(request.clone()).await.unwrap();
    ticket_policy(&f, &[]);
    for owner in [&embedded, &remote] {
        for request in [
            PluginHostRequest::ReadTicket {
                invocation_id: id.clone(),
                ticket_id: before.meta.id.clone(),
            },
            get(id, &before.meta.id, address("retained")),
            request.clone(),
            lookup(id, &before.meta.id, address("retained"), "write"),
        ] {
            assert_eq!(
                owner.call(request).await.unwrap_err().code,
                PluginHostErrorCode::NotFound
            );
        }
    }
    assert_eq!(
        host.read_ticket(host.context(), binding.as_ref())
            .await
            .unwrap_err()
            .code,
        errors::Code::NotFound
    );
    ticket_policy(&f, &[(&before.meta.id, true, true)]);
    assert!(matches!(
        remote
            .call(get(id, &before.meta.id, address("retained")))
            .await
            .unwrap(),
        PluginHostResponse::DataCell {
            cell: PluginDataCell::Present { .. }
        }
    ));
    f.revoke();
    for owner in [&embedded, &remote] {
        assert_denied(
            owner
                .call(get(id, &before.meta.id, address("retained")))
                .await,
        );
        assert_denied(
            owner
                .call(lookup(id, &before.meta.id, address("retained"), "write"))
                .await,
        );
    }
    let retained: i64 = f.api.config_store.with_conn(|conn| Ok(conn.query_row(
        "SELECT count(*) FROM plugin_data_cells WHERE workspace_id=?1 AND installation_id=?2",
        rusqlite::params![TEST_WORKSPACE_ID, f.installed.installation.installation_id], |row| row.get(0),
    )?)).unwrap();
    assert_eq!(
        retained, 1,
        "revoke fences execution without deleting extension data"
    );
    assert_ticket_unchanged(&f, &before);
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn ticket_handles_cannot_cross_owner_host_even_for_the_same_durable_invocation() {
    let f = data_fixture(true, true).await;
    let ticket = create_ticket(&f, "Opaque handle subject");
    ticket_policy(&f, &[(&ticket.meta.id, true, true)]);
    let embedded = f.owner(None).await;
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let first = data_host(&f, embedded, "same-call").await;
    let second = data_host(&f, remote, "same-call").await;
    assert_eq!(
        first.context().invocation_id,
        second.context().invocation_id
    );
    let binding = first
        .resolve_ticket(first.context(), &ticket.meta.id, objects::Access::Read)
        .await
        .unwrap();
    assert_eq!(
        second
            .read_ticket(second.context(), binding.as_ref())
            .await
            .unwrap_err()
            .code,
        errors::Code::Denied
    );
    let data = second
        .ticket_data(
            second.context(),
            binding.as_ref(),
            TicketDataOperation::Get(address("cell")),
        )
        .await;
    assert!(matches!(data, Err(error) if error.code == errors::Code::Denied));
    let read_only_write = first
        .ticket_data(
            first.context(),
            binding.as_ref(),
            TicketDataOperation::Mutate {
                address: address("read-only-handle"),
                operation_key: "write".into(),
                mutation: PluginDataMutation::Put {
                    expected: PluginDataExpectation::NeverWritten,
                    value_json: r#"{"text":"not authorized by read handle"}"#.into(),
                },
            },
        )
        .await;
    assert!(matches!(read_only_write, Err(error) if error.code == errors::Code::Denied));
    assert!(
        first
            .read_ticket(first.context(), binding.as_ref())
            .await
            .is_ok()
    );
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn mutation_response_loss_recovers_from_reopened_store_and_expiry_is_not_absence() {
    let f = data_fixture(true, true).await;
    let before = create_ticket(&f, "Receipt recovery does not mutate the Ticket");
    ticket_policy(&f, &[(&before.meta.id, true, true)]);
    let embedded = f.owner(None).await;
    let host = data_host(&f, embedded.clone(), "lost-mutation-reply").await;
    let id = &host.context().invocation_id;
    let request = put(
        id,
        &before.meta.id,
        address("durable"),
        "lost-reply",
        PluginDataExpectation::NeverWritten,
        "committed",
    );
    assert_eq!(
        embedded
            .call(lookup(
                id,
                &before.meta.id,
                address("durable"),
                "lost-reply"
            ))
            .await
            .unwrap(),
        PluginHostResponse::DataMutationLookup {
            outcome: PluginDataMutationLookup::NotRecorded
        }
    );
    // The real router commits normally, but the generated client cannot accept
    // its response body. No fake owner or alternate authentication is involved.
    let (lossy, server) =
        remote_owner_for_with_limit(&f, &f.source.worker_id, TEST_WORKSPACE_ID, 1).await;
    let error = lossy.call(request.clone()).await.unwrap_err();
    assert_eq!(error.code, PluginHostErrorCode::Indeterminate);
    assert_eq!(
        error.effect,
        server_api::PluginEffectObservation::Indeterminate
    );
    assert_eq!(
        lossy
            .call(lookup(
                id,
                &before.meta.id,
                address("durable"),
                "lost-reply"
            ))
            .await
            .unwrap_err()
            .code,
        PluginHostErrorCode::Unavailable,
        "lookup transport failure must not become NotRecorded"
    );
    let mut recovered_api = f.api.clone();
    recovered_api.config_store =
        Arc::new(SqliteWorkspaceStore::open_for_test(&f.api.config.database_path).unwrap());
    let recovered: Arc<dyn PluginHostOwner> =
        Arc::new(plugin_host::EmbeddedPluginHostOwner::authenticated(
            Arc::new(recovered_api.clone()),
            TEST_WORKSPACE_ID.into(),
            Arc::new(SignedAuthorizer {
                signer: worker_runtime::auth::RuntimeRequestSourceSigner::from_identity(
                    &f.identity,
                ),
                worker_id: f.source.worker_id.clone(),
            }),
        ));
    let restored = data_host(&f, recovered.clone(), "lost-mutation-reply").await;
    assert_eq!(restored.context().invocation_id, *id);
    let PluginHostResponse::DataMutationLookup {
        outcome: PluginDataMutationLookup::Committed(saved),
    } = recovered
        .call(lookup(
            id,
            &before.meta.id,
            address("durable"),
            "lost-reply",
        ))
        .await
        .unwrap()
    else {
        panic!("the lost response must still have a durable receipt")
    };
    assert_eq!(
        receipt(recovered.call(request.clone()).await.unwrap()),
        saved
    );
    assert!(
        matches!(recovered.call(get(id, &before.meta.id, address("durable"))).await.unwrap(),
        PluginHostResponse::DataCell { cell: PluginDataCell::Present { last_write, .. } } if last_write == saved.mutation_id)
    );
    recovered_api
        .config_store
        .with_immediate_transaction(|tx| {
            let record = crate::plugin_receipt::lookup(tx, TEST_WORKSPACE_ID, id)?;
            crate::plugin_receipt::prune(tx, TEST_WORKSPACE_ID, record.expires_at_ms)
        })
        .unwrap();
    for key in ["lost-reply", "never-recorded"] {
        assert_eq!(
            recovered
                .call(lookup(id, &before.meta.id, address("durable"), key))
                .await
                .unwrap(),
            PluginHostResponse::DataMutationLookup {
                outcome: PluginDataMutationLookup::Expired
            }
        );
    }
    assert_eq!(
        recovered.call(request).await.unwrap_err().code,
        PluginHostErrorCode::ReceiptExpired
    );
    assert_ticket_unchanged(&f, &before);
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn data_read_and_write_require_independent_plan_and_live_ticket_permissions() {
    for (plan_read, plan_write) in [(true, false), (false, true)] {
        let f = data_fixture(plan_read, plan_write).await;
        let before = create_ticket(&f, "Independent read and write ceilings");
        ticket_policy(&f, &[(&before.meta.id, true, true)]);
        let embedded = f.owner(None).await;
        let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
        let host = data_host(&f, embedded.clone(), "plan-ceiling").await;
        let id = &host.context().invocation_id;
        for owner in [&embedded, &remote] {
            let read = owner
                .call(get(id, &before.meta.id, address("permissions")))
                .await;
            if plan_read {
                assert!(read.is_ok());
            } else {
                assert_eq!(read.unwrap_err().code, PluginHostErrorCode::NotFound);
            }
            let write = owner
                .call(put(
                    id,
                    &before.meta.id,
                    address("permissions"),
                    "write",
                    PluginDataExpectation::NeverWritten,
                    "value",
                ))
                .await;
            if plan_write {
                assert!(write.is_ok());
            } else {
                assert_eq!(write.unwrap_err().code, PluginHostErrorCode::NotFound);
            }
        }
        assert_ticket_unchanged(&f, &before);
        drop(server);
        f.shutdown().await;
    }
    let f = data_fixture(true, true).await;
    let before = create_ticket(&f, "Current Subject permissions are a second ceiling");
    let embedded = f.owner(None).await;
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let host = data_host(&f, remote.clone(), "subject-ceiling").await;
    let id = &host.context().invocation_id;
    ticket_policy(&f, &[(&before.meta.id, true, false)]);
    for owner in [&embedded, &remote] {
        assert!(
            owner
                .call(get(id, &before.meta.id, address("permissions")))
                .await
                .is_ok()
        );
        assert_eq!(
            owner
                .call(put(
                    id,
                    &before.meta.id,
                    address("permissions"),
                    "write",
                    PluginDataExpectation::NeverWritten,
                    "value"
                ))
                .await
                .unwrap_err()
                .code,
            PluginHostErrorCode::NotFound
        );
    }
    ticket_policy(&f, &[(&before.meta.id, false, true)]);
    for owner in [&embedded, &remote] {
        assert_eq!(
            owner
                .call(get(id, &before.meta.id, address("permissions")))
                .await
                .unwrap_err()
                .code,
            PluginHostErrorCode::NotFound
        );
        owner
            .call(put(
                id,
                &before.meta.id,
                address("permissions"),
                "write",
                PluginDataExpectation::NeverWritten,
                "value",
            ))
            .await
            .unwrap();
    }
    assert_ticket_unchanged(&f, &before);
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn object_value_rejected_by_declared_string_schema_does_not_save_cell_or_receipt() {
    let f = data_fixture(true, true).await;
    let before = create_ticket(&f, "Closed value schema");
    ticket_policy(&f, &[(&before.meta.id, true, true)]);
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let host = data_host(&f, remote.clone(), "invalid-data-value").await;
    let id = &host.context().invocation_id;
    let mut invalid = put(
        id,
        &before.meta.id,
        address("invalid"),
        "invalid",
        PluginDataExpectation::NeverWritten,
        "value",
    );
    if let PluginHostRequest::DataMutate {
        mutation: PluginDataMutation::Put { value_json, .. },
        ..
    } = &mut invalid
    {
        *value_json = json!({"text":"value", "undeclared":true}).to_string();
    }
    assert_eq!(
        remote.call(invalid).await.unwrap_err().code,
        PluginHostErrorCode::InvalidInput
    );
    assert_eq!(
        remote
            .call(get(id, &before.meta.id, address("invalid")))
            .await
            .unwrap(),
        PluginHostResponse::DataCell {
            cell: PluginDataCell::NeverWritten
        }
    );
    assert_eq!(
        remote
            .call(lookup(id, &before.meta.id, address("invalid"), "invalid"))
            .await
            .unwrap(),
        PluginHostResponse::DataMutationLookup {
            outcome: PluginDataMutationLookup::NotRecorded
        }
    );
    assert_ticket_unchanged(&f, &before);
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn same_artifact_in_second_installation_cannot_read_cells_or_reuse_list_cursor() {
    use manifest::plugin::*;
    let f = data_fixture(true, true).await;
    let before = create_ticket(&f, "Installation scoped data");
    ticket_policy(&f, &[(&before.meta.id, true, true)]);
    let embedded = f.owner(None).await;
    let first = data_host(&f, embedded.clone(), "first-installation").await;
    let id = &first.context().invocation_id;
    for key in ["a", "b"] {
        embedded
            .call(put(
                id,
                &before.meta.id,
                address(key),
                key,
                PluginDataExpectation::NeverWritten,
                key,
            ))
            .await
            .unwrap();
    }
    let PluginHostResponse::DataListed { page } = embedded
        .call(PluginHostRequest::DataList {
            invocation_id: id.clone(),
            ticket_id: before.meta.id.clone(),
            collection: collection("1"),
            cursor: None,
            limit: 1,
        })
        .await
        .unwrap()
    else {
        panic!("list expected")
    };
    let cursor = page.next_cursor.unwrap();
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("plugin.toml"), data_manifest()).unwrap();
    fs::write(dir.path().join("plugin.component.wasm"), guest_component()).unwrap();
    let limits = PluginPackageLimits::default();
    let package = pack_plugin_package(
        &read_plugin_directory(dir.path(), PluginSourceKind::Explicit, &limits).unwrap(),
        &limits,
    )
    .unwrap();
    let upload = f
        .api
        .config_store
        .plugin_upload(
            &ServerConfig::server_data_root_for_data_dir(&f.api.config.data_root)
                .join("plugin-artifacts"),
            TEST_WORKSPACE_ID,
            "fixture-owner",
            &package.bytes,
        )
        .unwrap();
    let installed = f
        .api
        .config_store
        .plugin_install(
            TEST_WORKSPACE_ID,
            "fixture-owner",
            &server_api::PluginInstallRequest {
                upload_id: upload.upload_id,
                alias: "second-installation".into(),
                operation_key: "second-install".into(),
            },
        )
        .unwrap();
    assert_eq!(
        installed.installation.artifacts[0].artifact_digest,
        f.installed.installation.artifacts[0].artifact_digest
    );
    commit_config_with_ticket_grants(
        &f.api,
        &installed,
        f.tool,
        None,
        Some(&configured_grants(true, true)),
    );
    let store = open_subjektiv_store(&f.api).unwrap();
    let mut permissions = store.subject_permissions(&f.subject).unwrap().unwrap();
    let more: Vec<_> = permissions
        .execution
        .iter()
        .filter_map(|p| match &p.resource {
            PermissionResource::PluginInstallation { workspace_id, .. } => Some(ScopedPermission {
                operation: p.operation.clone(),
                resource: PermissionResource::PluginInstallation {
                    workspace_id: workspace_id.clone(),
                    installation_id: installed.installation.installation_id.clone(),
                },
            }),
            _ => None,
        })
        .collect();
    permissions.execution.extend(more);
    store
        .set_subject_permissions(
            &f.subject,
            &SubjectPermissionsUpdate {
                operation_id: Uuid::now_v7().to_string(),
                expected_change_id: Some(permissions.change_id),
                execution: permissions.execution,
                delegation: vec![],
            },
        )
        .unwrap();
    let execution = f.api.plugin_execution_identity(&f.source).await.unwrap();
    let state = f
        .api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let plan = f
        .api
        .config_store
        .with_immediate_transaction(|tx| {
            let plan = crate::plugin_settings::resolve_worker_plugin_plan(
                tx,
                TEST_WORKSPACE_ID,
                &state.snapshot.digest,
                PROFILE,
                execution,
            )?;
            crate::plugin_plan::plugin_plan_create(tx, "fixture-host", "second-launch", &plan)
        })
        .unwrap();
    f.api
        .config_store
        .plugin_plan_transition(
            TEST_WORKSPACE_ID,
            &plan.plan_id,
            PlanPinState::Pending,
            PlanPinState::Active,
        )
        .unwrap();
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let PluginHostResponse::Invocation {
        invocation_id: second,
        ..
    } = remote
        .call(PluginHostRequest::Begin {
            plan_id: plan.plan_id,
            installation_id: installed.installation.installation_id,
            consumer: f.consumer("second-installation"),
        })
        .await
        .unwrap()
    else {
        panic!("invocation expected")
    };
    for owner in [&embedded, &remote] {
        assert_eq!(
            owner
                .call(get(&second, &before.meta.id, address("a")))
                .await
                .unwrap(),
            PluginHostResponse::DataCell {
                cell: PluginDataCell::NeverWritten
            }
        );
        assert_eq!(
            owner
                .call(PluginHostRequest::DataList {
                    invocation_id: second.clone(),
                    ticket_id: before.meta.id.clone(),
                    collection: collection("1"),
                    cursor: Some(cursor.clone()),
                    limit: 1,
                })
                .await
                .unwrap_err()
                .code,
            PluginHostErrorCode::InvalidInput
        );
    }
    assert_ticket_unchanged(&f, &before);
    drop(server);
    f.shutdown().await;
}

fn schema_artifact_manifest(version: &str) -> String {
    use manifest::plugin::{PluginPackageManifest, PluginRequestedGrant};
    let mut manifest: PluginPackageManifest = toml::from_str(include_str!(
        "../../plugin-runtime/tests/ticket-guest/plugin.toml"
    ))
    .unwrap();
    manifest.version = format!("{version}.0.0");
    for declaration in &mut manifest.data_collections {
        declaration.schema_version = version.into();
        if version == "2" {
            declaration.schema = json!({
                "type":"object", "properties":{"count":{"type":"integer"}},
                "required":["count"], "additionalProperties":false,
            });
        }
    }
    for grant in &mut manifest.requested_grants {
        if let PluginRequestedGrant::TicketDataRead { schema_version, .. }
        | PluginRequestedGrant::TicketDataWrite { schema_version, .. } = grant
        {
            *schema_version = version.into();
        }
    }
    toml::to_string(&manifest).unwrap()
}

fn schema_grants(version: &str) -> Value {
    json!({"ticket_read":true, "ticket_data_read":[collection(version)], "ticket_data_write":[collection(version)]})
}

fn package_manifest(manifest: &str) -> manifest::plugin::CanonicalPluginArtifact {
    use manifest::plugin::*;
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("plugin.toml"), manifest).unwrap();
    fs::write(dir.path().join("plugin.component.wasm"), guest_component()).unwrap();
    let limits = PluginPackageLimits::default();
    pack_plugin_package(
        &read_plugin_directory(dir.path(), PluginSourceKind::Explicit, &limits).unwrap(),
        &limits,
    )
    .unwrap()
}

async fn active_config_plan(f: &Fixture, operation: &str) -> PluginPlanCreateReceipt {
    let execution = f.api.plugin_execution_identity(&f.source).await.unwrap();
    let state = f
        .api
        .config_store
        .load_workspace_config(TEST_WORKSPACE_ID)
        .unwrap()
        .unwrap();
    let plan = f
        .api
        .config_store
        .with_immediate_transaction(|tx| {
            let plan = crate::plugin_settings::resolve_worker_plugin_plan(
                tx,
                TEST_WORKSPACE_ID,
                &state.snapshot.digest,
                PROFILE,
                execution,
            )?;
            crate::plugin_plan::plugin_plan_create(tx, "fixture-host", operation, &plan)
        })
        .unwrap();
    f.api
        .config_store
        .plugin_plan_transition(
            TEST_WORKSPACE_ID,
            &plan.plan_id,
            PlanPinState::Pending,
            PlanPinState::Active,
        )
        .unwrap();
    plan
}

#[tokio::test]
async fn same_installation_schema_upgrade_and_original_pin_restore_keep_cells_separate() {
    let f = Fixture::with_package(
        None,
        schema_artifact_manifest("1"),
        guest_component(),
        "ticket_test",
        Some(schema_grants("1")),
    )
    .await;
    let before = create_ticket_with_slug(
        &f,
        "Schema rollback must retain original namespace",
        Some("T-795"),
    );
    ticket_policy(&f, &[(&before.meta.id, true, true)]);
    let embedded = f.owner(None).await;
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let old_host = data_host(&f, embedded.clone(), "original-pin-call").await;
    let old_invocation = old_host.context().invocation_id.clone();
    let old_request = put(
        &old_invocation,
        &before.meta.id,
        address("cycle"),
        "original-write",
        PluginDataExpectation::NeverWritten,
        "original string",
    );
    let original = receipt(embedded.call(old_request.clone()).await.unwrap());
    f.api
        .config_store
        .plugin_plan_transition(
            TEST_WORKSPACE_ID,
            &f.plan.plan_id,
            PlanPinState::Active,
            PlanPinState::Restorable,
        )
        .unwrap();

    // A second immutable archive and catalog artifact under the SAME installation,
    // with a genuinely incompatible declared schema and an exact new plan pin.
    // It uses the same actual Tool Component; no replacement/fake guest source.
    let package = package_manifest(&schema_artifact_manifest("2"));
    assert_ne!(
        package.package_digest,
        f.component.identity().package_digest
    );
    let compiled = f
        .executor
        .compile(
            &package.bytes,
            &package.package_digest,
            &package.artifact_digest,
            package.artifact_size,
        )
        .unwrap();
    let upload = f
        .api
        .config_store
        .plugin_upload(
            &ServerConfig::server_data_root_for_data_dir(&f.api.config.data_root)
                .join("plugin-artifacts"),
            TEST_WORKSPACE_ID,
            "fixture-owner",
            &package.bytes,
        )
        .unwrap();
    let added = f
        .api
        .config_store
        .plugin_add_artifact(
            TEST_WORKSPACE_ID,
            "fixture-owner",
            &f.installed.installation.installation_id,
            &server_api::PluginAddArtifactRequest {
                upload_id: upload.upload_id,
                expected_last_operation: f.installed.operation_id.clone(),
                operation_key: "add-schema-two".into(),
            },
        )
        .unwrap();
    assert_eq!(
        added.installation.installation_id,
        f.installed.installation.installation_id
    );
    let artifact = added
        .installation
        .artifacts
        .iter()
        .find(|a| a.artifact_digest == package.artifact_digest)
        .unwrap();
    commit_config_for_artifact(
        &f.api,
        &added,
        artifact,
        f.tool,
        None,
        Some(&schema_grants("2")),
    );
    let upgraded = active_config_plan(&f, "schema-two-plan").await;
    assert_ne!(upgraded.plan_id, f.plan.plan_id);
    assert_eq!(
        upgraded.plan.plugins()[0].artifact_digest(),
        package.artifact_digest
    );
    let new_host = host_for_plan(&f, remote.clone(), &upgraded, &compiled, "schema-two-call").await;
    let new_invocation = new_host.context().invocation_id.clone();
    let mut new_address = address("cycle");
    new_address.collection = collection("2");
    assert_eq!(
        remote
            .call(get(&new_invocation, &before.meta.id, new_address.clone()))
            .await
            .unwrap(),
        PluginHostResponse::DataCell {
            cell: PluginDataCell::NeverWritten
        }
    );
    assert_denied(
        remote
            .call(get(&new_invocation, &before.meta.id, address("cycle")))
            .await,
    );
    let newer = receipt(
        remote
            .call(PluginHostRequest::DataMutate {
                invocation_id: new_invocation.clone(),
                ticket_id: before.meta.id.clone(),
                address: new_address.clone(),
                operation_key: "new-write".into(),
                mutation: PluginDataMutation::Put {
                    expected: PluginDataExpectation::NeverWritten,
                    value_json: json!({"count":2}).to_string(),
                },
            })
            .await
            .unwrap(),
    );
    assert_eq!(
        f.executor
            .invoke(
                &compiled,
                new_host.context().clone(),
                new_host,
                f.tool,
                r#""read""#,
                "{}"
            )
            .await
            .unwrap()
            .output
            .summary,
        before.meta.title
    );

    // Rollback chooses the original artifact in actual Workspace/Profile config,
    // and restores the retained ORIGINAL plan rather than synthesizing a pin
    // from the latest artifact or widening its schema grant set.
    let original_artifact = &f.installed.installation.artifacts[0];
    commit_config_for_artifact(
        &f.api,
        &added,
        original_artifact,
        f.tool,
        None,
        Some(&schema_grants("1")),
    );
    f.api
        .config_store
        .plugin_plan_transition(
            TEST_WORKSPACE_ID,
            &upgraded.plan_id,
            PlanPinState::Active,
            PlanPinState::Restorable,
        )
        .unwrap();
    let restored_plan = f
        .api
        .config_store
        .plugin_plan_transition(
            TEST_WORKSPACE_ID,
            &f.plan.plan_id,
            PlanPinState::Restorable,
            PlanPinState::Active,
        )
        .unwrap();
    assert_eq!(restored_plan.plan, f.plan.plan);
    assert_eq!(restored_plan.fingerprint, f.plan.fingerprint);
    remote
        .prepare(
            f.plan.plan_id.clone(),
            f.installed.installation.installation_id.clone(),
            vec![f.tool.into()],
        )
        .await
        .unwrap();
    let restored = data_host(&f, remote.clone(), "original-pin-call").await;
    assert_eq!(restored.context().invocation_id, old_invocation);
    assert_eq!(restored.context().artifact, *f.component.identity());
    assert_eq!(
        f.executor
            .invoke(
                &f.component,
                restored.context().clone(),
                restored,
                f.tool,
                r#""read""#,
                "{}"
            )
            .await
            .unwrap()
            .output
            .summary,
        before.meta.title
    );
    assert_eq!(receipt(remote.call(old_request).await.unwrap()), original);
    for owner in [&embedded, &remote] {
        assert_eq!(
            owner
                .call(get(&old_invocation, &before.meta.id, address("cycle")))
                .await
                .unwrap(),
            PluginHostResponse::DataCell {
                cell: PluginDataCell::Present {
                    value_json: r#""original string""#.into(),
                    last_write: original.mutation_id.clone()
                }
            }
        );
        assert_denied(
            owner
                .call(get(&old_invocation, &before.meta.id, new_address.clone()))
                .await,
        );
        assert_eq!(
            owner
                .call(get(&new_invocation, &before.meta.id, new_address.clone()))
                .await
                .unwrap(),
            PluginHostResponse::DataCell {
                cell: PluginDataCell::Present {
                    value_json: json!({"count":2}).to_string(),
                    last_write: newer.mutation_id.clone()
                }
            }
        );
    }
    assert_ticket_unchanged(&f, &before);
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn foreign_workspace_or_invisible_ticket_resolve_is_not_found() {
    let f = data_fixture(true, true).await;
    let local = create_ticket(&f, "Invisible local Ticket");
    let mut workspace = f
        .api
        .store
        .get_workspace(TEST_WORKSPACE_ID)
        .await
        .unwrap()
        .unwrap();
    workspace.workspace_id = "foreign-data-workspace".into();
    f.api.store.upsert_workspace(&workspace).await.unwrap();
    let foreign = ticket::SqliteTicketBackend::open_verified(
        &f.api.config.database_path,
        &workspace.workspace_id,
    )
    .unwrap()
    .create(ticket::NewTicket::new("Foreign Ticket"))
    .unwrap();
    let embedded = f.owner(None).await;
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let host = data_host(&f, embedded.clone(), "hidden-tickets").await;
    let id = &host.context().invocation_id;
    for owner in [&embedded, &remote] {
        for reference in [
            local.meta.id.as_str(),
            foreign.id.as_str(),
            "missing-ticket",
        ] {
            assert_eq!(
                owner
                    .call(PluginHostRequest::ResolveTicket {
                        invocation_id: id.clone(),
                        ticket_ref: reference.into(),
                        access: PluginTicketAccess::Read,
                    })
                    .await
                    .unwrap_err()
                    .code,
                PluginHostErrorCode::NotFound
            );
        }
    }
    let (foreign_owner, foreign_server) =
        remote_owner_for(&f, &f.source.worker_id, &workspace.workspace_id).await;
    assert!(
        foreign_owner
            .call(get(id, &local.meta.id, address("a")))
            .await
            .is_err()
    );
    assert_ticket_unchanged(&f, &local);
    drop(foreign_server);
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn ticket_snapshot_is_utf8_bounded_and_omits_thread_fields() {
    let f = data_fixture(true, false).await;
    let body = format!(
        "{}終端",
        "あ".repeat(server_api::PLUGIN_TICKET_BODY_LIMIT / 3 + 1)
    );
    let before = create_ticket(&f, &body);
    ticket_policy(&f, &[(&before.meta.id, true, false)]);
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let host = data_host(&f, remote.clone(), "bounded-read").await;
    let response = remote
        .call(PluginHostRequest::ReadTicket {
            invocation_id: host.context().invocation_id.clone(),
            ticket_id: before.meta.id.clone(),
        })
        .await
        .unwrap();
    let PluginHostResponse::TicketRead { snapshot } = &response else {
        panic!("snapshot expected")
    };
    assert!(snapshot.truncated);
    assert!(snapshot.body.len() <= server_api::PLUGIN_TICKET_BODY_LIMIT);
    assert!(body.starts_with(&snapshot.body));
    assert_eq!(snapshot.case_state, PluginTicketCaseState::Open);
    assert_eq!(
        snapshot.content_digest,
        ticket::ticket_content_digest(&before)
    );
    let json = serde_json::to_value(response).unwrap();
    let fields = json["data"]["snapshot"].as_object().unwrap();
    assert_eq!(fields.len(), 5);
    assert!(!json.to_string().contains("private thread"));
    assert_ticket_unchanged(&f, &before);
    drop(server);
    f.shutdown().await;
}

#[tokio::test]
async fn administrator_purge_routes_require_post_revoke_snapshot_exact_cas_and_live_authority() {
    let f = data_fixture(true, true).await;
    let before = create_ticket(&f, "Purge must not change this Ticket.");
    ticket_policy(&f, &[(&before.meta.id, true, true)]);
    let owner = f.owner(None).await;
    let host = data_host(&f, owner.clone(), "admin-purge-cell").await;
    owner
        .call(put(
            &host.context().invocation_id,
            &before.meta.id,
            address("admin"),
            "admin-write",
            PluginDataExpectation::NeverWritten,
            "retained",
        ))
        .await
        .unwrap();
    let manager = seed_test_workspace_owner_token(f.api.store.as_ref(), "data-manager").await;
    let reader = seed_test_api_token(f.api.store.as_ref(), "data-reader");
    let app = build_router(f.api.clone());
    let prefix = format!(
        "/api/w/{TEST_WORKSPACE_ID}/plugins/{}",
        f.installed.installation.installation_id
    );
    let observe = format!("{prefix}/data-state");
    let purge = format!("{prefix}/data-purge");
    use crate::server::tests::plugin_tests::request;
    request(
        app.clone(),
        Some(&reader),
        "GET",
        &observe,
        vec![],
        false,
        StatusCode::FORBIDDEN,
    )
    .await;
    request(
        app.clone(),
        Some(&manager),
        "GET",
        &observe,
        vec![],
        false,
        StatusCode::CONFLICT,
    )
    .await;
    let revoke = serde_json::to_vec(&server_api::PluginRevokeRequest {
        artifact_digest: f.installed.installation.artifacts[0]
            .artifact_digest
            .clone(),
        expected_last_operation: f.installed.installation.last_operation.clone(),
        reason: "Explicit data purge confirmation".into(),
        operation_key: "revoke-before-observe".into(),
    })
    .unwrap();
    request(
        app.clone(),
        Some(&manager),
        "POST",
        &format!("{prefix}/revoke"),
        revoke,
        false,
        StatusCode::OK,
    )
    .await;
    let state = request(
        app.clone(),
        Some(&manager),
        "GET",
        &observe,
        vec![],
        false,
        StatusCode::OK,
    )
    .await;
    assert_eq!(state["cell_count"], 1);
    let mut wrong = state.clone();
    wrong["cells_digest"] = json!(format!("sha256:{}", "0".repeat(64)));
    request(
        app.clone(),
        Some(&manager),
        "POST",
        &purge,
        serde_json::to_vec(&json!({"operation_key":"purge", "expected":wrong})).unwrap(),
        false,
        StatusCode::CONFLICT,
    )
    .await;
    assert_eq!(f.count("plugin_data_cells"), 1);
    let body = serde_json::to_vec(&json!({"operation_key":"purge", "expected":state})).unwrap();
    let receipt = request(
        app.clone(),
        Some(&manager),
        "POST",
        &purge,
        body.clone(),
        false,
        StatusCode::OK,
    )
    .await;
    assert_eq!(receipt["observed"], state);
    assert_eq!(f.count("plugin_data_cells"), 0);
    assert_eq!(f.count("plugin_data_receipts"), 1);
    assert_eq!(f.count("plugin_data_purges"), 1);
    assert_eq!(
        request(
            app.clone(),
            Some(&manager),
            "POST",
            &purge,
            body.clone(),
            false,
            StatusCode::OK
        )
        .await,
        receipt
    );
    request(
        app,
        Some(&reader),
        "POST",
        &purge,
        body,
        false,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_ticket_unchanged(&f, &before);
    f.shutdown().await;
}

#[tokio::test]
async fn remote_maximum_escaped_value_page_fits_exported_response_budget_and_small_client_fails() {
    let mut manifest: manifest::plugin::PluginPackageManifest =
        toml::from_str(&data_manifest()).unwrap();
    for declaration in &mut manifest.data_collections {
        declaration.schema = json!({"type":"string", "maxLength":65536});
    }
    let f = Fixture::with_package(
        None,
        toml::to_string(&manifest).unwrap(),
        guest_component(),
        "ticket_test",
        Some(configured_grants(true, true)),
    )
    .await;
    let ticket = create_ticket(&f, "Bounded page");
    ticket_policy(&f, &[(&ticket.meta.id, true, true)]);
    let embedded = f.owner(None).await;
    let host = data_host(&f, embedded.clone(), "escaped-page").await;
    let invocation = &host.context().invocation_id;
    // Each encoded value is exactly 64 KiB, with nearly every byte escaped
    // again by the Remote envelope. The full 100-entry page remains bounded.
    let text = "\n".repeat((server_api::PLUGIN_DATA_VALUE_LIMIT - 2) / 2);
    assert_eq!(
        json!(text).to_string().len(),
        server_api::PLUGIN_DATA_VALUE_LIMIT
    );
    for index in 0..server_api::PLUGIN_DATA_LIST_LIMIT {
        let key = format!("{index:03}-{}", "k".repeat(252));
        embedded
            .call(put(
                invocation,
                &ticket.meta.id,
                address(&key),
                &format!("page-{index}"),
                PluginDataExpectation::NeverWritten,
                &text,
            ))
            .await
            .unwrap();
    }
    let request = PluginHostRequest::DataList {
        invocation_id: invocation.clone(),
        ticket_id: ticket.meta.id.clone(),
        collection: collection("1"),
        cursor: None,
        limit: server_api::PLUGIN_DATA_LIST_LIMIT,
    };
    let (remote, server) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
    let response = remote.call(request.clone()).await.unwrap();
    assert!(
        serde_json::to_vec(&response).unwrap().len()
            < yoi_plugin_runtime::owner::PLUGIN_HOST_RESPONSE_BODY_LIMIT
    );
    let PluginHostResponse::DataListed { page } = response else {
        panic!("page expected")
    };
    assert_eq!(
        page.entries.len(),
        server_api::PLUGIN_DATA_LIST_LIMIT as usize
    );
    assert!(page.next_cursor.is_none());
    for entry in page.entries {
        assert!(
            matches!(entry.cell, PluginDataCell::Present { value_json, .. } if value_json.len() == server_api::PLUGIN_DATA_VALUE_LIMIT)
        );
    }
    // A caller-selected smaller transport budget rejects the reply; no first
    // page or NotRecorded result is synthesized on that transport failure.
    let (small, small_server) =
        remote_owner_for_with_limit(&f, &f.source.worker_id, TEST_WORKSPACE_ID, 1024).await;
    assert_eq!(
        small.call(request).await.unwrap_err().code,
        PluginHostErrorCode::Unavailable
    );
    drop(small_server);
    drop(server);
    assert_ticket_unchanged(&f, &ticket);
    f.shutdown().await;
}
