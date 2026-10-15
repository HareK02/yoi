//! Bounded wire projection of canonical public material, never Memory previews.

use runtime_api::*;
use session_store::public_material::{
    PublicMaterialDescriptor, PublicMaterialError, SessionPublicMaterialReader,
};
use sha2::{Digest, Sha256};

use crate::{Runtime, RuntimeWorkspaceScope, worker_backend::SessionMaterialSourceError};

fn descriptor(
    value: &PublicMaterialDescriptor,
) -> Result<SessionMaterialDescriptor, PublicMaterialError> {
    let result = SessionMaterialDescriptor {
        segment_id: value.identity.segment_id.to_string(),
        entry_ref: value.identity.entry_ref.clone(),
        canonical_order: value.canonical_order,
        origin_json: serde_json::to_string(&value.origin)?,
        sha256: value.sha256.clone(),
        total_bytes: value.total_bytes,
    };
    if serde_json::to_vec(&result)?.len() > 16 * 1024 {
        return Err(PublicMaterialError::ResourceLimit);
    }
    Ok(result)
}

fn catalog_digest(reader: &SessionPublicMaterialReader) -> Result<String, PublicMaterialError> {
    let mut digest = Sha256::new();
    digest.update(reader.session_id().to_string());
    digest.update(reader.active_segment_id().to_string());
    for value in reader.descriptors() {
        digest.update(serde_json::to_vec(&descriptor(value)?)?);
        digest.update(b"\n");
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn unavailable(error: SessionMaterialSourceError) -> SessionPublicUnavailableReason {
    use PublicMaterialError as E;
    use SessionPublicUnavailableReason as R;
    match error {
        SessionMaterialSourceError::Material(E::ResourceLimit) => R::ResourceLimit,
        SessionMaterialSourceError::Material(E::Changed | E::InvalidOffset) => R::InvalidCursor,
        SessionMaterialSourceError::Material(E::Missing | E::Erased) => R::RetentionMissing,
        SessionMaterialSourceError::Material(E::Inconsistent | E::Record(_)) => R::CorruptLog,
        _ => R::StorageUnavailable,
    }
}

pub(crate) fn catalog(
    runtime: &Runtime,
    scope: &RuntimeWorkspaceScope,
    request: &SessionMaterialCatalogRequest,
) -> SessionMaterialCatalogAvailability {
    if scope.workspace_id == request.workspace_id
        && (1..=100).contains(&request.limit)
        && request.offset == 0
        && request.expected_catalog_sha256.is_none()
        && runtime
            .session_not_materialized_scoped(scope, &request.source, &request.expected_session_id)
            .is_ok_and(|empty| empty)
    {
        return SessionMaterialCatalogAvailability::NotMaterialized {
            session_id: request.expected_session_id.clone(),
        };
    }
    let result = (|| -> Result<_, SessionMaterialSourceError> {
        if scope.workspace_id != request.workspace_id {
            return Err("Session Workspace scope mismatch".into());
        }
        if request.limit == 0 || request.limit > 100 {
            return Err(PublicMaterialError::ResourceLimit.into());
        }
        let reader = runtime.session_public_material_reader_scoped(
            scope,
            &request.source,
            &request.expected_session_id,
        )?;
        let digest = catalog_digest(&reader)?;
        if request
            .expected_catalog_sha256
            .as_ref()
            .is_some_and(|expected| expected != &digest)
        {
            return Err(PublicMaterialError::Changed.into());
        }
        let offset =
            usize::try_from(request.offset).map_err(|_| PublicMaterialError::InvalidOffset)?;
        let count = reader.descriptors().len();
        if offset > count {
            return Err(PublicMaterialError::InvalidOffset.into());
        }
        let descriptors = reader
            .descriptors()
            .skip(offset)
            .take(request.limit)
            .map(descriptor)
            .collect::<Result<Vec<_>, _>>()?;
        let end = offset + descriptors.len();
        Ok(SessionMaterialCatalogPage {
            session_id: reader.session_id().to_string(),
            active_segment_id: reader.active_segment_id().to_string(),
            catalog_sha256: digest,
            descriptors,
            next_offset: (end < count).then_some(end as u64),
        })
    })();
    match result {
        Ok(page) => SessionMaterialCatalogAvailability::Page { page },
        Err(error) => SessionMaterialCatalogAvailability::Unavailable {
            reason: unavailable(error),
            message: "Canonical Session catalog unavailable".into(),
        },
    }
}

pub(crate) fn read(
    runtime: &Runtime,
    scope: &RuntimeWorkspaceScope,
    request: &SessionMaterialReadRequest,
) -> SessionMaterialReadAvailability {
    let result = (|| -> Result<_, SessionMaterialSourceError> {
        if scope.workspace_id != request.workspace_id {
            return Err("Session Workspace scope mismatch".into());
        }
        if !(4..=16 * 1024).contains(&request.max_content_bytes) {
            return Err(PublicMaterialError::ResourceLimit.into());
        }
        let reader = runtime.session_public_material_reader_scoped(
            scope,
            &request.source,
            &request.expected_session_id,
        )?;
        // Resolve saved identity/order/digest here; no caller-supplied descriptor is adopted.
        // New tail entries may arrive during a Job without changing its frozen originals.
        let selected = reader
            .descriptors()
            .find(|value| {
                value.identity.segment_id.to_string() == request.expected_descriptor.segment_id
                    && value.identity.entry_ref == request.expected_descriptor.entry_ref
            })
            .ok_or(PublicMaterialError::Missing)?;
        if descriptor(selected)? != request.expected_descriptor {
            return Err(PublicMaterialError::Changed.into());
        }
        let content =
            reader.read_content(selected, request.byte_offset, request.max_content_bytes)?;
        Ok(SessionMaterialReadPage {
            descriptor: descriptor(selected)?,
            content: content.content,
            next_offset: content.next_offset,
        })
    })();
    match result {
        Ok(page) => SessionMaterialReadAvailability::Page { page },
        Err(error) => SessionMaterialReadAvailability::Unavailable {
            reason: unavailable(error),
            message: "Canonical Session material unavailable".into(),
        },
    }
}
