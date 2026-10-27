//! Explicit, operation-owned repair of saved Session material. This is not
//! read-only candidate preparation: the canonical Session/attribution is the
//! receipt for replay after a lost response, and effects are reported as such.

use super::*;

impl ProfileRuntimeWorkerFactory {
    pub(super) async fn recover_saved_restore_source(
        &self,
        request: &WorkerExecutionRestoreRequest,
    ) -> Result<bool, RuntimeRestoreFailure> {
        let cause = |cause| RuntimeRestoreFailure::started(RuntimeRestoreStage::Recovery, cause);
        let name = Self::runtime_worker_name_for_ref(&request.worker_ref);
        let root = self
            .worker_aggregate_dir(&request.worker_ref)
            .map_err(|_| cause("restore recovery aggregate unavailable"))?;
        let metadata_store = WorkerAggregateStore::open_read_only(&root, &name)
            .map_err(|_| cause("restore recovery metadata open failed"))?;
        let metadata = metadata_store
            .read_by_name(&name)
            .map_err(|_| cause("restore recovery metadata read failed"))?
            .ok_or_else(|| cause("restore recovery metadata missing"))?;
        let active = metadata
            .active
            .as_ref()
            .ok_or_else(|| cause("restore recovery Session missing"))?;
        let pending = active.segment_id.is_none();
        let incomplete = request.request.subjektiv_attached
            && !matches!(
                metadata.subjektiv_session_attribution,
                Some(session_store::SubjektivSessionAttributionState::Confirmed { .. })
            );
        if !pending && !incomplete {
            return Ok(false);
        }
        request
            .context
            .authorize_restore_source_repair(request.operation_id)
            .map_err(|_| cause("canonical source repair authority is not durably confirmed"))?;
        // The accepted Runtime operation already owns this Session. Its stable
        // Session identity and sole committed seed, not a new restore request,
        // are used to reconcile an interrupted first-head publication.
        let session_store = WorkerSessionStore::new_with_retention(
            root.join("session"),
            self.session_retention()
                .map_err(|_| cause("restore recovery retention unavailable"))?,
        )
        .map_err(|_| cause("restore recovery Session open failed"))?;
        let (manifest, mut loader) = Self::manifest_for_restore(&metadata, &request.request)
            .map_err(|_| cause("restore recovery saved manifest invalid"))?;
        let workspace = RuntimeWorkspaceBackendRef::from_worker_request(
            &request.request,
            self.runtime_id.as_deref(),
        )
        .map_err(cause)?;
        #[cfg(feature = "http-server")]
        let request_client = request
            .request
            .workspace_api
            .as_ref()
            .and_then(|api| self.workspace_request_clients.get(&api.workspace_id));
        let context = workspace.worker_context(
            #[cfg(feature = "http-server")]
            &request.worker_ref,
            #[cfg(feature = "http-server")]
            request.workspace_scope.as_ref(),
            #[cfg(feature = "http-server")]
            self.worker_mutation_identity.as_ref(),
            #[cfg(feature = "http-server")]
            request_client,
            #[cfg(feature = "http-server")]
            self.embedded_worker_mutation_dispatcher.as_ref(),
            #[cfg(feature = "http-server")]
            Some(self.prompt_projection_cache.clone()),
        );
        if pending && context.workspace_id().is_some() {
            let bundle = request
                .config_bundle
                .as_ref()
                .ok_or_else(|| cause("pending recovery launch material missing"))?;
            let projection = self
                .observe_bundle_prompt_projection(
                    bundle,
                    request
                        .request
                        .workspace_api
                        .as_ref()
                        .map(|api| api.workspace_id.as_str()),
                )
                .map_err(|_| cause("pending recovery Prompt projection invalid"))?
                .ok_or_else(|| cause("pending recovery Prompt projection missing"))?;
            loader = loader.with_effective_catalog(projection.projection.catalog.clone());
        }
        self.failed_restore_resources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                request.worker_ref.clone(),
                FailedRestoreResources {
                    operation_id: request.operation_id,
                    sessions: Vec::new(),
                    candidate_workdir_cleanup: None,
                    allocation: None,
                    feature_tasks: None,
                },
            );
        let store = CombinedStore::new(session_store, metadata_store);
        let worker_result = Worker::restore_pending_from_worker_metadata_with_context(
            &name,
            manifest,
            store,
            loader,
            context,
            WorkerFilesystemAuthority::None,
        )
        .await;
        let mut worker = worker_result.map_err(|error| {
            self.retain_construction_failure(request, &error);
            cause(restore_worker_cause(&error))
        })?;
        let tasks = worker.retain_startup_feature_cleanup();
        let sessions = worker.workdir_sessions();
        let allocation = worker.restore_allocation_cleanup_handle();
        self.failed_restore_resources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                request.worker_ref.clone(),
                FailedRestoreResources {
                    operation_id: request.operation_id,
                    sessions: vec![sessions.clone()],
                    candidate_workdir_cleanup: None,
                    allocation: allocation.clone(),
                    feature_tasks: Some(tasks.clone()),
                },
            );
        let result = async {
            if let Some(binding) = request.request.backend_job.as_ref() {
                worker
                    .bind_backend_job(binding.clone())
                    .map_err(|_| cause("recovery Backend Job binding failed"))?;
            }
            worker
                .finalize_subjektiv_session_attribution(
                    request.request.subjektiv_attached,
                    SubjektivSessionAttributionLifecycle::RestoredSession,
                )
                .map_err(|error| cause(restore_worker_cause(&error)))?;
            if pending {
                worker::WorkerController::recover_pending_head(&mut worker, &root)
                    .await
                    .map_err(|_| cause("pending Session head recovery failed"))?;
            }
            Ok::<_, RuntimeRestoreFailure>(())
        }
        .await;
        // Never release allocation while the Worker guard is attached. Retained
        // handles in the same operation remain available on every cleanup error.
        drop(worker);
        let cleanup = async {
            tasks
                .shutdown()
                .await
                .map_err(|_| cause("recovery Feature settlement failed"))?;
            sessions
                .close_all()
                .await
                .map_err(|_| cause("recovery Workdir close failed"))?;
            if let Some(allocation) = allocation {
                allocation
                    .release()
                    .map_err(|_| cause("recovery allocation release failed"))?;
            }
            Ok::<_, RuntimeRestoreFailure>(())
        }
        .await;
        result?;
        cleanup?;
        self.failed_restore_resources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&request.worker_ref);
        Ok(true)
    }
}
