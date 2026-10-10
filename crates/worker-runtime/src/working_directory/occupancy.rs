//! Transient, clone-shared admission for local filesystem effects. Bindings are
//! passive; only effects and activated session resources own leases. The mutex
//! protects admission bookkeeping only, never filesystem work.
use super::WorkingDirectoryDiagnostic;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub(super) struct Occupancy {
    entries: Arc<Mutex<HashMap<String, Entry>>>,
    generations: Arc<Mutex<HashMap<String, Arc<()>>>>,
}

/// Passive bindings identify a materialization generation, not merely its ID.
#[derive(Clone, Debug)]
pub(super) struct Generation(Arc<()>);

#[derive(Debug)]
enum Entry {
    Use(usize),
    Create,
    Cleanup,
}

#[derive(Debug)]
pub(super) struct Lease {
    occupancy: Occupancy,
    id: String,
}

fn busy() -> WorkingDirectoryDiagnostic {
    WorkingDirectoryDiagnostic::new(
        "working_directory_cleanup_resource_busy",
        "A filesystem resource is busy or changed during removal. Release the resource and retry.",
    )
}

impl Occupancy {
    pub(super) fn acquire_use(&self, id: &str) -> Result<Lease, WorkingDirectoryDiagnostic> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match entries.get_mut(id) {
            Some(Entry::Cleanup | Entry::Create) => return Err(busy()),
            Some(Entry::Use(count)) => *count += 1,
            None => {
                entries.insert(id.to_string(), Entry::Use(1));
            }
        }
        Ok(Lease {
            occupancy: self.clone(),
            id: id.to_string(),
        })
    }

    pub(super) fn generation(&self, id: &str) -> Generation {
        let mut generations = self
            .generations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Generation(
            generations
                .entry(id.to_string())
                .or_insert_with(|| Arc::new(()))
                .clone(),
        )
    }

    // Called only after successful removal, while exclusive cleanup is held.
    pub(super) fn invalidate_generation(&self, id: &str) {
        self.generations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(id);
    }

    pub(super) fn acquire_session_use(
        &self,
        id: &str,
        generation: &Generation,
    ) -> Result<Lease, WorkingDirectoryDiagnostic> {
        let lease = self.acquire_use(id)?;
        let valid = self
            .generations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(id)
            .is_some_and(|current| Arc::ptr_eq(current, &generation.0));
        if !valid {
            return Err(WorkingDirectoryDiagnostic::new(
                "working_directory_not_found",
                "Workdir binding is stale; acquire a fresh binding before opening a session",
            ));
        }
        Ok(lease)
    }

    pub(super) fn acquire_creation(&self, id: &str) -> Result<Lease, WorkingDirectoryDiagnostic> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if entries.contains_key(id) {
            return Err(busy());
        }
        entries.insert(id.to_string(), Entry::Create);
        Ok(Lease {
            occupancy: self.clone(),
            id: id.to_string(),
        })
    }

    pub(super) fn acquire_cleanup(&self, id: &str) -> Result<Lease, WorkingDirectoryDiagnostic> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if entries.contains_key(id) {
            return Err(busy());
        }
        entries.insert(id.to_string(), Entry::Cleanup);
        Ok(Lease {
            occupancy: self.clone(),
            id: id.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_and_cleanup_leases_release_during_unwind() {
        let occupancy = Occupancy::default();
        for operation in ["use", "create", "cleanup"] {
            let result = std::panic::catch_unwind(|| {
                let _lease = match operation {
                    "cleanup" => occupancy.acquire_cleanup("workdir-panic").unwrap(),
                    "create" => occupancy.acquire_creation("workdir-panic").unwrap(),
                    _ => occupancy.acquire_use("workdir-panic").unwrap(),
                };
                panic!("effect failed after admission");
            });
            assert!(result.is_err());
            drop(occupancy.acquire_use("workdir-panic").unwrap());
            drop(occupancy.acquire_cleanup("workdir-panic").unwrap());
        }
    }

    #[test]
    fn simultaneous_use_and_cleanup_admit_only_one_direction() {
        let occupancy = Occupancy::default();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            let use_occupancy = occupancy.clone();
            let use_barrier = barrier.clone();
            let use_thread = scope.spawn(move || {
                use_barrier.wait();
                let result = use_occupancy.acquire_use("workdir-race");
                // Keep the winner's lease until both admission attempts finish.
                use_barrier.wait();
                result
            });
            barrier.wait();
            let cleanup = occupancy.acquire_cleanup("workdir-race");
            barrier.wait();
            let use_result = use_thread.join().unwrap();
            assert_ne!(use_result.is_ok(), cleanup.is_ok());
            let error = if let Err(error) = use_result {
                error
            } else {
                cleanup.unwrap_err()
            };
            assert_eq!(error.code, "working_directory_cleanup_resource_busy");
        });
        drop(occupancy.acquire_cleanup("workdir-race").unwrap());
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut entries = self
            .occupancy
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match entries.get_mut(&self.id) {
            Some(Entry::Use(count)) if *count > 1 => *count -= 1,
            _ => {
                entries.remove(&self.id);
            }
        }
    }
}
