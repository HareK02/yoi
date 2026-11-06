//! Local execution uses the current source and registered Feature namespace;
//! retained body/Job grants remain independent domain restrictions.
use super::*;
use ::feature_permissions::{NamespaceId, PermissionKey};
use server_api::authorization::{PermissionResource, ScopedPermission};
use worker::feature::FeatureId;

impl LocalSubjectHost {
    pub(super) fn freeze_operations(
        &self,
        operations: &[&str],
    ) -> Result<Vec<String>, SubjektivHostError> {
        let mut granted = Vec::new();
        for operation in operations {
            match self.authorize_current(operation) {
                Ok(()) => granted.push((*operation).to_owned()),
                Err(SubjektivHostError::PermissionDenied(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(granted)
    }

    pub(super) fn authorize_current(&self, operation: &str) -> Result<(), SubjektivHostError> {
        self.ensure_live()?;
        self.check_current_policy(operation)?;
        self.ensure_live()
    }

    pub(super) fn check_current_policy(&self, operation: &str) -> Result<(), SubjektivHostError> {
        let registry = self
            .catalog
            .permissions
            .lock()
            .expect("Subject permissions poisoned");
        let subject = self
            .catalog
            .store
            .subject(&self.binding.subject_id)
            .map_err(domain_error)?;
        if subject.is_none_or(|subject| subject.state != subjektiv::SubjectState::Active) {
            return Err(SubjektivHostError::PermissionDenied(
                "Subject is not active".into(),
            ));
        }
        let source = self
            .catalog
            .store
            .current_subject_policy(&self.binding.subject_id)
            .map_err(domain_error)?
            .ok_or_else(|| {
                SubjektivHostError::PermissionDenied("Subject permissions are unset".into())
            })?;
        let key = PermissionKey::parse(operation).map_err(SubjektivHostError::PermissionDenied)?;
        // These are the three service boundaries implemented by this Host, not
        // a caller-supplied Feature selector. Other Features check their own handles.
        let owner = match key.namespace().as_str() {
            "memory" => "memory",
            "journal" => "journal-recording",
            "session" => "session-explore",
            _ => {
                return Err(SubjektivHostError::PermissionDenied(
                    "operation is not a local Subject service".into(),
                ));
            }
        };
        let namespace = registry
            .namespace_handle(
                &FeatureId::builtin(owner),
                &NamespaceId::parse(key.namespace().as_str())
                    .map_err(SubjektivHostError::InvalidArgument)?,
            )
            .ok_or_else(|| {
                SubjektivHostError::Unavailable("Subject Feature namespace is unavailable".into())
            })?;
        let resolved = registry.host().resolve(Some(&source.execution));
        if resolved.projection().diagnostics.iter().any(|diagnostic| {
            matches!(diagnostic, ::feature_permissions::PermissionDiagnostic::DisabledFeature(feature)
                if feature.as_str() == FeatureId::builtin(owner).as_str())
        }) {
            return Err(SubjektivHostError::Unavailable("Subject Feature is disabled".into()));
        }
        let allowed = registry
            .host()
            .allows(namespace, &resolved, &key)
            .map_err(|error| SubjektivHostError::PermissionDenied(error.to_string()))?;
        if !allowed {
            return Err(SubjektivHostError::PermissionDenied(format!(
                "current Subject policy does not grant `{operation}`"
            )));
        }
        if let Some(legacy) = source.legacy_constraints {
            let requested = ScopedPermission {
                operation: operation.into(),
                resource: PermissionResource::Subject {
                    workspace_id: self.binding.scope_id.clone(),
                    subject_id: self.binding.subject_id.clone(),
                },
            };
            if !legacy
                .execution
                .iter()
                .any(|permission| permission.covers(&requested))
            {
                return Err(SubjektivHostError::PermissionDenied(
                    "legacy Subject target restriction does not cover this operation".into(),
                ));
            }
        }
        Ok(())
    }
}
