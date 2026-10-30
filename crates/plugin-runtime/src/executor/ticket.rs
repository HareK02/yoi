use super::*;
use crate::host::{
    Ticket, TicketDataOperation as Op, TicketDataResult as Reply, extension_data as data, objects,
    ticket_read,
};
use server_api::{
    PluginDataAddress as Address, PluginDataCell as Cell, PluginDataCollection as Collection,
    PluginDataExpectation as Expectation, PluginDataMutation as Mutation,
};

fn denied() -> errors::HostError {
    host::error(errors::Code::Denied, "Ticket capability not granted")
}
fn invalid() -> errors::HostError {
    host::error(errors::Code::InvalidInput, "invalid Ticket data request")
}
fn protocol() -> errors::HostError {
    host::error(errors::Code::Unavailable, "invalid Ticket owner response")
}
fn collection(c: data::Collection) -> Collection {
    Collection {
        collection: c.collection,
        schema_id: c.schema_id,
        schema_version: c.schema_version,
    }
}
fn address(a: data::Address) -> Address {
    Address {
        collection: collection(a.namespace),
        key: a.key,
    }
}
fn cell(c: Cell) -> data::Cell {
    match c {
        Cell::NeverWritten => data::Cell::NeverWritten,
        Cell::Deleted { last_delete } => data::Cell::Deleted(last_delete),
        Cell::Present {
            value_json,
            last_write,
        } => data::Cell::Present(data::PresentCell {
            value_json,
            last_write,
        }),
    }
}
fn receipt(r: server_api::PluginDataReceipt) -> data::Receipt {
    data::Receipt {
        mutation_id: r.mutation_id,
        result_digest: r.result_digest,
    }
}
impl State {
    fn ticket_world(&self) -> Result<(), errors::HostError> {
        if self.manifest.runtime.world != plugin::PLUGIN_COMPONENT_TICKET_TOOL_WORLD {
            return Err(denied());
        }
        Ok(())
    }
    fn data_schema(&self, c: &Collection) -> Result<&serde_json::Value, errors::HostError> {
        self.manifest
            .data_collections
            .iter()
            .find(|d| {
                d.collection == c.collection
                    && d.schema_id == c.schema_id
                    && d.schema_version == c.schema_version
            })
            .map(|d| &d.schema)
            .ok_or_else(denied)
    }
    fn data_grant(&self, ctx: &InvocationContext, c: &Collection, write: bool) -> bool {
        let id = (
            c.collection.clone(),
            c.schema_id.clone(),
            c.schema_version.clone(),
        );
        let requested = if write {
            plugin::PluginRequestedGrant::TicketDataWrite {
                collection: c.collection.clone(),
                schema_id: c.schema_id.clone(),
                schema_version: c.schema_version.clone(),
            }
        } else {
            plugin::PluginRequestedGrant::TicketDataRead {
                collection: c.collection.clone(),
                schema_id: c.schema_id.clone(),
                schema_version: c.schema_version.clone(),
            }
        };
        self.manifest.requested_grants.contains(&requested)
            && (if write {
                &ctx.grants.ticket_data_write
            } else {
                &ctx.grants.ticket_data_read
            })
            .contains(&id)
    }
    fn check_cell(&self, namespace: &Collection, c: &Cell) -> Result<(), errors::HostError> {
        if let Cell::Present { value_json, .. } = c {
            if value_json.len() > server_api::PLUGIN_DATA_VALUE_LIMIT {
                return Err(protocol());
            }
            let value = serde_json::from_str(value_json).map_err(|_| protocol())?;
            plugin::schema::validate_data_value(self.data_schema(namespace)?, &value)
                .map_err(|_| protocol())?;
        }
        Ok(())
    }
    async fn data_call(
        &mut self,
        target: Resource<Ticket>,
        operation: Op,
    ) -> wasmtime::Result<Result<Reply, errors::HostError>> {
        let context = self.current()?;
        let checked = (|| {
            self.operation()?;
            self.ticket_world()?;
            crate::owner::validate_operation(&operation).map_err(|_| invalid())?;
            let (namespace, write, lookup) = match &operation {
                Op::Get(a) => (&a.collection, false, false),
                Op::Mutate { address, .. } => (&address.collection, true, false),
                Op::List { collection, .. } => (collection, false, false),
                Op::LookupMutation { address, .. } => (&address.collection, false, true),
            };
            let schema = self.data_schema(namespace)?;
            if !self.data_grant(&context, namespace, write || lookup) {
                return Err(denied());
            }
            if let Op::Mutate {
                mutation: Mutation::Put { value_json, .. },
                ..
            } = &operation
            {
                let value = serde_json::from_str(value_json).map_err(|_| invalid())?;
                plugin::schema::validate_data_value(schema, &value).map_err(|_| invalid())?;
            }
            Ok(())
        })();
        if let Err(e) = checked {
            return Ok(Err(e));
        }
        let binding = self.table.get(&target)?.binding.clone();
        let namespace = match &operation {
            Op::Get(a) | Op::Mutate { address: a, .. } | Op::LookupMutation { address: a, .. } => {
                a.collection.clone()
            }
            Op::List { collection, .. } => collection.clone(),
        };
        let list_limit = if let Op::List { limit, .. } = &operation {
            Some(*limit as usize)
        } else {
            None
        };
        let mutating = matches!(&operation, Op::Mutate { .. });
        let expected_reply = match &operation {
            Op::Get(_) => 0,
            Op::Mutate { .. } => 1,
            Op::List { .. } => 2,
            Op::LookupMutation { .. } => 3,
        };
        let reply = match self
            .provider
            .ticket_data(&context, binding.as_ref(), operation)
            .await
        {
            Ok(reply) => reply,
            Err(e) => return Ok(Err(e)),
        };
        check_current(&context).map_err(|_| wasmtime::Error::msg("late Ticket data result"))?;
        let actual_reply = match &reply {
            Reply::Cell(_) => 0,
            Reply::Mutated(_) => 1,
            Reply::Listed(_) => 2,
            Reply::MutationLookup(_) => 3,
        };
        if actual_reply != expected_reply {
            let mut error = protocol();
            if mutating {
                error.code = errors::Code::Indeterminate;
                error.effect = errors::EffectObservation::Indeterminate;
            }
            return Ok(Err(error));
        }
        let valid = match &reply {
            Reply::Cell(c) => self.check_cell(&namespace, c),
            Reply::Listed(page) => {
                if page.entries.len() > list_limit.unwrap_or(0)
                    || page.entries.windows(2).any(|p| p[0].key >= p[1].key)
                    || page.entries.iter().any(|e| {
                        e.key.is_empty()
                            || e.key.len() > 256
                            || matches!(e.cell, Cell::NeverWritten)
                    })
                    || page
                        .next_cursor
                        .as_ref()
                        .is_some_and(|s| s.is_empty() || s.len() > 4096)
                {
                    Err(protocol())
                } else {
                    page.entries
                        .iter()
                        .try_for_each(|e| self.check_cell(&namespace, &e.cell))
                }
            }
            _ => Ok(()),
        };
        if let Err(e) = valid {
            return Ok(Err(e));
        }
        Ok(Ok(reply))
    }
}
impl objects::HostTicket for State {
    async fn drop(&mut self, target: Resource<Ticket>) -> wasmtime::Result<()> {
        self.table.delete(target)?;
        self.resources -= 1;
        Ok(())
    }
}
impl objects::Host for State {
    async fn resolve_ticket(
        &mut self,
        ticket_ref: String,
        requested: objects::Access,
    ) -> wasmtime::Result<Result<Resource<Ticket>, errors::HostError>> {
        let context = self.current()?;
        if let Err(e) = self.operation().and_then(|_| self.ticket_world()) {
            return Ok(Err(e));
        }
        if ticket_ref.is_empty() || ticket_ref.len() > 256 {
            return Ok(Err(invalid()));
        }
        let writable = matches!(requested, objects::Access::ExtensionWrite);
        let allowed = self.manifest.requested_grants.iter().any(|g| match g {
            plugin::PluginRequestedGrant::TicketRead => !writable && context.grants.ticket_read,
            plugin::PluginRequestedGrant::TicketDataRead {
                collection,
                schema_id,
                schema_version,
            } => {
                !writable
                    && context.grants.ticket_data_read.contains(&(
                        collection.clone(),
                        schema_id.clone(),
                        schema_version.clone(),
                    ))
            }
            plugin::PluginRequestedGrant::TicketDataWrite {
                collection,
                schema_id,
                schema_version,
            } => {
                writable
                    && context.grants.ticket_data_write.contains(&(
                        collection.clone(),
                        schema_id.clone(),
                        schema_version.clone(),
                    ))
            }
            _ => false,
        });
        if !allowed {
            return Ok(Err(denied()));
        }
        if self.resources >= self.limits.resources {
            return Ok(Err(host::error(
                errors::Code::QuotaExceeded,
                "resource limit",
            )));
        }
        let binding = match self
            .provider
            .resolve_ticket(&context, &ticket_ref, requested)
            .await
        {
            Ok(binding) => binding,
            Err(e) => return Ok(Err(e)),
        };
        check_current(&context).map_err(|_| wasmtime::Error::msg("late Ticket resource"))?;
        let target = self.table.push(Ticket { binding })?;
        self.resources += 1;
        Ok(Ok(target))
    }
}
impl ticket_read::Host for State {
    async fn read(
        &mut self,
        target: Resource<Ticket>,
    ) -> wasmtime::Result<Result<ticket_read::Snapshot, errors::HostError>> {
        let context = self.current()?;
        if let Err(e) = self.operation().and_then(|_| self.ticket_world()) {
            return Ok(Err(e));
        }
        if !context.grants.ticket_read
            || !self
                .manifest
                .requested_grants
                .contains(&plugin::PluginRequestedGrant::TicketRead)
        {
            return Ok(Err(denied()));
        }
        let binding = self.table.get(&target)?.binding.clone();
        let result = self.provider.read_ticket(&context, binding.as_ref()).await;
        check_current(&context).map_err(|_| wasmtime::Error::msg("late Ticket snapshot"))?;
        Ok(result.and_then(|s| {
            if s.body.len() <= server_api::PLUGIN_TICKET_BODY_LIMIT {
                Ok(s)
            } else {
                Err(protocol())
            }
        }))
    }
}
impl data::Host for State {
    async fn get(
        &mut self,
        target: Resource<Ticket>,
        cell_address: data::Address,
    ) -> wasmtime::Result<Result<data::Cell, errors::HostError>> {
        Ok(
            match self
                .data_call(target, Op::Get(address(cell_address)))
                .await?
            {
                Ok(Reply::Cell(c)) => Ok(cell(c)),
                Err(e) => Err(e),
                _ => Err(protocol()),
            },
        )
    }
    async fn put(
        &mut self,
        target: Resource<Ticket>,
        cell_address: data::Address,
        expected: data::Expectation,
        operation_key: String,
        value_json: String,
    ) -> wasmtime::Result<Result<data::Receipt, errors::HostError>> {
        let expected = match expected {
            data::Expectation::NeverWritten => Expectation::NeverWritten,
            data::Expectation::LastWrite(id) => Expectation::LastWrite(id),
        };
        Ok(
            match self
                .data_call(
                    target,
                    Op::Mutate {
                        address: address(cell_address),
                        operation_key,
                        mutation: Mutation::Put {
                            expected,
                            value_json,
                        },
                    },
                )
                .await?
            {
                Ok(Reply::Mutated(r)) => Ok(receipt(r)),
                Err(e) => Err(e),
                _ => Err(protocol()),
            },
        )
    }
    async fn delete(
        &mut self,
        target: Resource<Ticket>,
        cell_address: data::Address,
        expected_last_write: String,
        operation_key: String,
    ) -> wasmtime::Result<Result<data::Receipt, errors::HostError>> {
        Ok(
            match self
                .data_call(
                    target,
                    Op::Mutate {
                        address: address(cell_address),
                        operation_key,
                        mutation: Mutation::Delete {
                            expected_last_write,
                        },
                    },
                )
                .await?
            {
                Ok(Reply::Mutated(r)) => Ok(receipt(r)),
                Err(e) => Err(e),
                _ => Err(protocol()),
            },
        )
    }
    async fn list(
        &mut self,
        target: Resource<Ticket>,
        namespace: data::Collection,
        cursor: Option<String>,
        limit: u32,
    ) -> wasmtime::Result<Result<data::Page, errors::HostError>> {
        Ok(
            match self
                .data_call(
                    target,
                    Op::List {
                        collection: collection(namespace),
                        cursor,
                        limit,
                    },
                )
                .await?
            {
                Ok(Reply::Listed(p)) => Ok(data::Page {
                    entries: p
                        .entries
                        .into_iter()
                        .map(|e| data::Entry {
                            key: e.key,
                            value: cell(e.cell),
                        })
                        .collect(),
                    next_cursor: p.next_cursor,
                }),
                Err(e) => Err(e),
                _ => Err(protocol()),
            },
        )
    }
    async fn lookup_mutation(
        &mut self,
        target: Resource<Ticket>,
        cell_address: data::Address,
        operation_key: String,
    ) -> wasmtime::Result<Result<data::MutationLookup, errors::HostError>> {
        use server_api::PluginDataMutationLookup as Lookup;
        Ok(
            match self
                .data_call(
                    target,
                    Op::LookupMutation {
                        address: address(cell_address),
                        operation_key,
                    },
                )
                .await?
            {
                Ok(Reply::MutationLookup(outcome)) => Ok(match outcome {
                    Lookup::NotRecorded => data::MutationLookup::NotRecorded,
                    Lookup::Expired => data::MutationLookup::Expired,
                    Lookup::Committed(r) => data::MutationLookup::Committed(receipt(r)),
                }),
                Err(e) => Err(e),
                _ => Err(protocol()),
            },
        )
    }
}
