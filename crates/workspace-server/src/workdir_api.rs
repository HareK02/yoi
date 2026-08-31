use workdir::workspace::{MaterializerKind, WorkingDirectoryStatusKind};
use workspace_api::{
    WorkingDirectoryCleanupTarget, WorkingDirectoryMaterializerKind, WorkingDirectoryOccupancy,
    WorkingDirectoryStatusKind as ApiWorkingDirectoryStatusKind, WorkingDirectorySummary,
};

pub(crate) fn summary(
    source: workdir::workspace::WorkingDirectorySummary,
) -> WorkingDirectorySummary {
    WorkingDirectorySummary {
        working_directory_id: source.working_directory_id,
        repository_id: source.repository_id,
        creation_selector: source.creation_selector,
        creation_ref: source.creation_ref,
        creation_tree: source.creation_tree,
        current_selector: source.current_selector,
        current_ref: source.current_ref,
        current_tree: source.current_tree,
        observed_at_epoch_seconds: source.observed_at_epoch_seconds,
        materializer_kind: match source.materializer_kind {
            MaterializerKind::RuntimeGitCache => WorkingDirectoryMaterializerKind::RuntimeGitCache,
            MaterializerKind::LocalGitWorktree => {
                WorkingDirectoryMaterializerKind::LocalGitWorktree
            }
        },
        cleanup_target: source
            .cleanup_target
            .map(|target| WorkingDirectoryCleanupTarget {
                kind: target.kind,
                working_directory_id: target.working_directory_id,
                repository_id: target.repository_id,
            }),
        status: match source.status {
            WorkingDirectoryStatusKind::Active => ApiWorkingDirectoryStatusKind::Active,
            WorkingDirectoryStatusKind::CleanupPending => {
                ApiWorkingDirectoryStatusKind::CleanupPending
            }
            WorkingDirectoryStatusKind::Corrupted => ApiWorkingDirectoryStatusKind::Corrupted,
            WorkingDirectoryStatusKind::NotFound => ApiWorkingDirectoryStatusKind::NotFound,
            WorkingDirectoryStatusKind::Unknown => ApiWorkingDirectoryStatusKind::Unknown,
        },
        cleanliness: source.cleanliness,
        primary_worker_id: source.primary_worker_id,
        occupied_by: source
            .occupied_by
            .map(|occupancy| WorkingDirectoryOccupancy {
                runtime_id: occupancy.worker.runtime_id,
                worker_id: occupancy.worker.worker_id,
                display_name: occupancy.display_name,
                linked_at: occupancy.linked_at,
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workdir::workspace::{
        RuntimeWorkerRef, WorkingDirectoryOccupancy as DomainOccupancy,
        WorkingDirectorySummary as DomainSummary,
    };

    #[test]
    fn conversion_preserves_public_occupancy_subject() {
        let converted = summary(DomainSummary {
            working_directory_id: "workdir-1".into(),
            repository_id: "main".into(),
            creation_selector: None,
            creation_ref: None,
            creation_tree: None,
            current_selector: None,
            current_ref: None,
            current_tree: None,
            observed_at_epoch_seconds: None,
            materializer_kind: MaterializerKind::RuntimeGitCache,
            cleanup_target: None,
            status: WorkingDirectoryStatusKind::Active,
            cleanliness: None,
            primary_worker_id: None,
            occupied_by: Some(DomainOccupancy {
                worker: RuntimeWorkerRef::new("arcadia", "worker-1"),
                display_name: "Coder".into(),
                linked_at: "2026-01-01T00:00:00Z".into(),
            }),
        });

        let occupancy = converted.occupied_by.expect("occupancy");
        assert_eq!(occupancy.runtime_id, "arcadia");
        assert_eq!(occupancy.worker_id, "worker-1");
    }
}
