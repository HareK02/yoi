//! Aggregate Store budgets: Wasmtime's built-in memory/table sizes are per
//! object, so count accepted growth across all objects as well. Nothing in a
//! Tool Store outlives that invocation; no cross-Store accounting is retained.
use crate::Limits;
use wasmtime::{ResourceLimiter, StoreLimits, StoreLimitsBuilder};

pub(crate) struct SandboxLimits {
    inner: StoreLimits,
    memory: Budget,
    table: Budget,
}
struct Budget {
    used: usize,
    last_growth: usize,
    ceiling: usize,
}
impl Budget {
    fn new(ceiling: usize) -> Self {
        Self {
            used: 0,
            last_growth: 0,
            ceiling,
        }
    }
    fn reserve(&mut self, current: usize, desired: usize, kind: &str) -> wasmtime::Result<()> {
        let growth = desired.saturating_sub(current);
        if growth > self.ceiling.saturating_sub(self.used) {
            return Err(wasmtime::Error::msg(format!(
                "aggregate {kind} budget exceeded"
            )));
        }
        self.used += growth;
        self.last_growth = growth;
        Ok(())
    }
    fn failed(&mut self) {
        self.used -= self.last_growth;
        self.last_growth = 0;
    }
}
impl SandboxLimits {
    pub(crate) fn new(limits: &Limits) -> Self {
        Self {
            inner: StoreLimitsBuilder::new()
                .memory_size(limits.memory_bytes)
                .table_elements(limits.table_elements)
                .instances(limits.instances)
                .memories(limits.memories)
                .tables(limits.tables)
                .trap_on_grow_failure(true)
                .build(),
            memory: Budget::new(limits.memory_bytes),
            table: Budget::new(limits.table_elements),
        }
    }
}
impl ResourceLimiter for SandboxLimits {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if !self.inner.memory_growing(current, desired, maximum)? {
            return Ok(false);
        }
        self.memory.reserve(current, desired, "memory")?;
        Ok(true)
    }
    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if !self.inner.table_growing(current, desired, maximum)? {
            return Ok(false);
        }
        self.table.reserve(current, desired, "table")?;
        Ok(true)
    }
    fn memory_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.memory.failed();
        self.inner.memory_grow_failed(error)
    }
    fn table_grow_failed(&mut self, error: wasmtime::Error) -> wasmtime::Result<()> {
        self.table.failed();
        self.inner.table_grow_failed(error)
    }
    fn instances(&self) -> usize {
        self.inner.instances()
    }
    fn memories(&self) -> usize {
        self.inner.memories()
    }
    fn tables(&self) -> usize {
        self.inner.tables()
    }
}
