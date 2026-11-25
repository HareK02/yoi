//! Backend connection adapters use the Worker Feature planner, not a second resolver.
//! Payload meaning and protected effects remain with the owning adapter. The
//! transport persists resolved records on the ordinary create reservation.
use super::*;
use protocol::feature_initialization::{FeatureInitialization, FeatureInitializations};
use worker::feature::{FeatureDescriptor, ServiceId, ServiceRequirement, ServiceVersionReq};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CreationAuthority {
    Ordinary,
    FeatureOwned,
}

pub(super) struct ConnectionCaller<'a> {
    pub api: &'a WorkspaceApi,
    pub source: Option<&'a RuntimeWorkerRef>,
    pub account: Option<&'a str>,
    pub authorization: &'a server_api::launch_policy::WorkerCreationAuthorization,
    pub singleton_key: Option<&'a str>,
}

pub(super) struct ConnectionResolution {
    pub initialization: FeatureInitialization,
    pub singleton_key: Option<String>,
}

/// A reserved identity is evidence, not a grant. Feature hooks must consult
/// current Host/domain authority before invoking protected Services. T-883 can
/// consume this seam without extending common create DTOs or grant fields.
pub(super) struct ReservedConnection<'a> {
    pub api: &'a WorkspaceApi,
    pub worker: &'a RuntimeWorkerRef,
    pub allocation_key: &'a str,
    pub operation: Option<&'a WorkerControlOperation>,
    pub reservation: &'a crate::store::WorkerCreateReservation,
}

pub(super) trait BackendFeatureInitializationAdapter: Send + Sync {
    fn feature_id(&self) -> &'static str;
    fn version(&self) -> u32;
    fn descriptor(&self) -> FeatureDescriptor;
    fn authorize(
        &self,
        caller: &ConnectionCaller<'_>,
        request: &FeatureInitialization,
    ) -> ApiResult<CreationAuthority>;
    fn resolve(
        &self,
        api: &WorkspaceApi,
        profile: &manifest::WorkerManifest,
        request: &FeatureInitialization,
    ) -> ApiResult<ConnectionResolution>;
    fn validate_resolved(
        &self,
        api: &WorkspaceApi,
        request: &WorkerSpawnRequest,
        initialization: &FeatureInitialization,
    ) -> ApiResult<CreationAuthority>;
    fn admit_reservation(
        &self,
        _api: &WorkspaceApi,
        _initialization: &FeatureInitialization,
        reserve: &mut dyn FnMut() -> ApiResult<crate::store::WorkerCreateReservation>,
    ) -> ApiResult<crate::store::WorkerCreateReservation> {
        reserve()
    }
    fn reserved(
        &self,
        _context: &ReservedConnection<'_>,
        _initialization: &FeatureInitialization,
    ) -> ApiResult<()> {
        Ok(())
    }
    fn accepted(
        &self,
        _api: &WorkspaceApi,
        _initialization: &FeatureInitialization,
        result: WorkerSpawnResult,
    ) -> ApiResult<WorkerSpawnResult> {
        Ok(result)
    }
}

pub(super) fn initialization_service(feature: &str) -> ServiceId {
    ServiceId::new(format!("backend-initialization:{feature}"))
        .expect("trusted Feature service identifier")
}

pub(super) struct BackendInitializationRegistry {
    adapters: Vec<Arc<dyn BackendFeatureInitializationAdapter>>,
}

impl BackendInitializationRegistry {
    pub fn new(adapters: Vec<Arc<dyn BackendFeatureInitializationAdapter>>) -> Self {
        Self { adapters }
    }

    fn selected<'a>(
        &'a self,
        records: &'a FeatureInitializations,
    ) -> ApiResult<
        Vec<(
            &'a dyn BackendFeatureInitializationAdapter,
            &'a FeatureInitialization,
        )>,
    > {
        let mut descriptors = self
            .adapters
            .iter()
            .map(|a| a.descriptor())
            .collect::<Vec<_>>();
        let mut consumer = FeatureDescriptor::builtin(
            "backend-initialization-consumer",
            "Backend connection requests",
        );
        for record in records.iter() {
            consumer = consumer.with_service_requirement(
                ServiceRequirement::required(
                    initialization_service(record.feature_id()),
                    "requested Worker connection",
                )
                .with_version(ServiceVersionReq::exact(record.version().to_string())),
            );
        }
        descriptors.push(consumer);
        let plan = worker::feature::build_feature_install_plan(&descriptors).map_err(|e| {
            Error::InvalidInput(format!("Backend Feature initialization plan: {e}"))
        })?;
        let mut selected = Vec::new();
        for record in records.iter() {
            let provider = plan
                .service_providers()
                .get(&initialization_service(record.feature_id()))
                .ok_or_else(|| {
                    Error::InvalidInput(format!(
                        "missing Backend initialization provider for {}",
                        record.feature_id()
                    ))
                })?;
            let adapter = self
                .adapters
                .iter()
                .find(|a| a.descriptor().id == provider.provider)
                .ok_or_else(|| {
                    Error::Config(
                        "selected Backend initialization provider is not registered".into(),
                    )
                })?;
            if adapter.feature_id() != record.feature_id() || adapter.version() != record.version()
            {
                return Err(Error::InvalidInput(
                    "Backend initialization provider contract mismatch".into(),
                )
                .into());
            }
            selected.push((adapter.as_ref(), record));
        }
        selected.sort_by_key(|(adapter, _)| {
            plan.ordered_features()
                .iter()
                .position(|id| *id == adapter.descriptor().id)
        });
        Ok(selected)
    }

    pub fn authorize(
        &self,
        caller: &ConnectionCaller<'_>,
        records: &FeatureInitializations,
    ) -> ApiResult<CreationAuthority> {
        let mut authority = CreationAuthority::Ordinary;
        for (adapter, record) in self.selected(records)? {
            if adapter.authorize(caller, record)? == CreationAuthority::FeatureOwned {
                authority = CreationAuthority::FeatureOwned;
            }
        }
        Ok(authority)
    }

    pub fn resolve(
        &self,
        api: &WorkspaceApi,
        profile: &manifest::WorkerManifest,
        records: &FeatureInitializations,
        singleton: &mut Option<String>,
    ) -> ApiResult<FeatureInitializations> {
        let mut resolved = Vec::new();
        for (adapter, record) in self.selected(records)? {
            let connection = adapter.resolve(api, profile, record)?;
            if let Some(key) = connection.singleton_key {
                if singleton.is_some() {
                    return Err(Error::InvalidInput(
                        "multiple or caller-supplied singleton identities for Feature connection"
                            .into(),
                    )
                    .into());
                }
                crate::store::validate_worker_singleton_key(&key)?;
                *singleton = Some(key);
            }
            resolved.push(connection.initialization);
        }
        FeatureInitializations::new(resolved).map_err(|e| Error::InvalidInput(e).into())
    }

    pub fn validate_resolved(
        &self,
        api: &WorkspaceApi,
        request: &WorkerSpawnRequest,
    ) -> ApiResult<CreationAuthority> {
        let records = &request.resolved_feature_initialization;
        // A requested connection cannot disappear between public resolution and reservation.
        for record in request.feature_connections.iter() {
            if records.get(record.feature_id()).is_none() {
                return Err(Error::InvalidInput(format!(
                    "missing resolved initialization for {}",
                    record.feature_id()
                ))
                .into());
            }
        }
        let mut authority = CreationAuthority::Ordinary;
        for (adapter, record) in self.selected(records)? {
            if adapter.validate_resolved(api, request, record)? == CreationAuthority::FeatureOwned {
                authority = CreationAuthority::FeatureOwned;
            }
        }
        Ok(authority)
    }

    pub fn reserve(
        &self,
        api: &WorkspaceApi,
        records: &FeatureInitializations,
        reserve: &mut dyn FnMut() -> ApiResult<crate::store::WorkerCreateReservation>,
    ) -> ApiResult<crate::store::WorkerCreateReservation> {
        fn next(
            selected: &[(
                &dyn BackendFeatureInitializationAdapter,
                &FeatureInitialization,
            )],
            api: &WorkspaceApi,
            reserve: &mut dyn FnMut() -> ApiResult<crate::store::WorkerCreateReservation>,
        ) -> ApiResult<crate::store::WorkerCreateReservation> {
            match selected.split_first() {
                Some(((adapter, record), rest)) => {
                    adapter.admit_reservation(api, record, &mut || next(rest, api, reserve))
                }
                None => reserve(),
            }
        }
        next(&self.selected(records)?, api, reserve)
    }

    pub fn reserved(&self, context: &ReservedConnection<'_>) -> ApiResult<()> {
        let records = &context.reservation.feature_initialization;
        for (adapter, record) in self.selected(records)? {
            adapter.reserved(context, record)?;
        }
        Ok(())
    }

    pub fn accepted(
        &self,
        api: &WorkspaceApi,
        records: &FeatureInitializations,
        mut result: WorkerSpawnResult,
    ) -> ApiResult<WorkerSpawnResult> {
        for (adapter, record) in self.selected(records)? {
            result = adapter.accepted(api, record, result)?;
        }
        Ok(result)
    }
}

/// Only the trusted composition root selects implementations; callers select
/// bounded records, never a provider object or a concrete implementation name.
pub(super) fn backend_initialization_registry() -> BackendInitializationRegistry {
    BackendInitializationRegistry::new(vec![Arc::new(
        super::subjektiv_worker_initialization::SubjektivBackendAdapter,
    )])
}
