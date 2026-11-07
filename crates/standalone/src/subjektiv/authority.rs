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
        let registry = self
            .catalog
            .permissions
            .lock()
            .expect("Subject permissions poisoned");
        self.check_policy_with_registry(operation, &source, &registry)
    }

    fn check_policy_with_registry(
        &self,
        operation: &str,
        source: &server_api::authorization::CurrentSubjectPolicy,
        registry: &worker::feature::permissions::FeaturePermissionRegistry,
    ) -> Result<(), SubjektivHostError> {
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
        if let Some(legacy) = &source.legacy_constraints {
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

// Shared test-only canonical owner for native-admission schedules. Production Local
// policy remains in the admitted Feature DB; tests disclose this substitution.
#[cfg(test)]
pub(super) struct MutablePolicy {
    pub(super) workspace: String,
    subject: String,
    pub(super) current: Mutex<Option<server_api::authorization::CurrentSubjectPolicy>>,
}
#[cfg(test)]
impl subjektiv::CanonicalPermissionSource for MutablePolicy {
    fn subject_permissions(
        &self,
        _: &str,
        _: &str,
    ) -> subjektiv::Result<Option<server_api::authorization::SubjectPermissions>> {
        panic!("the local consumer must not use the legacy policy projection")
    }
    fn current_subject_policy(
        &self,
        workspace: &str,
        subject: &str,
    ) -> subjektiv::Result<Option<server_api::authorization::CurrentSubjectPolicy>> {
        assert_eq!(workspace, self.workspace);
        assert_eq!(subject, self.subject);
        Ok(self.current.lock().unwrap().clone())
    }
}
#[cfg(test)]
pub(super) fn bind_canonical_policy(
    catalog: &mut StandaloneSubjects,
    subject: &SubjectRecord,
) -> Arc<MutablePolicy> {
    let source = Arc::new(MutablePolicy {
        workspace: catalog.scope_id.clone(),
        subject: subject.id.clone(),
        current: Mutex::new(catalog.store.current_subject_policy(&subject.id).unwrap()),
    });
    let identity = catalog.store.database_identity().unwrap();
    let preview = catalog
        .store
        .preview_permission_transfer(&identity)
        .unwrap();
    catalog
        .store
        .seal_permission_sources(&identity, "local-test-transfer", &preview.export_digest)
        .unwrap();
    catalog.store = catalog
        .store
        .clone()
        .with_canonical_permission_source(source.clone());
    source
}

pub(super) struct LocalMemoryEffectAuthority {
    pub(super) host: Arc<LocalSubjectHost>,
    pub(super) operation: String,
}
impl subjektiv::effect_admission::SubjectEffectAuthority for LocalMemoryEffectAuthority {
    fn enter<'a>(
        &'a self,
        admission: &subjektiv::effect_admission::SubjectEffectAdmission<'_>,
    ) -> subjektiv::Result<Box<dyn 'a + subjektiv::effect_admission::SubjectEffectLease>> {
        if !self.host.live.load(Ordering::Acquire) || !admission.writer_active(&self.host.writer)? {
            return Err(subjektiv::SubjektivError::AuthorizationDenied(
                "Subject writer is no longer active".into(),
            ));
        }
        let policy = admission
            .current_policy(&self.host.binding.subject_id)?
            .ok_or_else(|| {
                subjektiv::SubjektivError::AuthorizationDenied("Subject policy is unset".into())
            })?;
        let registry = self
            .host
            .catalog
            .permissions
            .lock()
            .expect("Subject permissions poisoned");
        self.host
            .check_policy_with_registry(&self.operation, &policy, &registry)
            .map_err(|error| subjektiv::SubjektivError::AuthorizationDenied(error.to_string()))?;
        Ok(Box::new(registry))
    }
}
impl LocalSubjectHost {
    pub(super) fn memory_effect_store(
        &self,
        operation: &str,
    ) -> Result<subjektiv::SubjektivStore, SubjektivHostError> {
        let host = self
            .self_weak
            .upgrade()
            .ok_or_else(|| SubjektivHostError::Unavailable("Subject Host is closed".into()))?;
        Ok(self
            .catalog
            .store
            .with_subject_effect_authority(Arc::new(LocalMemoryEffectAuthority {
                host,
                operation: operation.into(),
            })))
    }
}
