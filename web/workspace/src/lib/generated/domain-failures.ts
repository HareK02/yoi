// @generated from Rust domain cause schemas and FailureCause identity. Do not edit.
// Regenerate/check: cargo run -q -p yoi-workspace-server --example generate_domain_failure_types -- [--check]

export type ConfigFailureDetails = { operation: ConfigFailureOperation; reason: ConfigFailureReason; stage: ConfigFailureStage; };

export type ConfigFailureOperation = "current" | "attach" | "observe" | "read" | "commit" | "post";

export type ConfigFailureReason = "authorization_denied" | "authorization_failed" | "invalid_request" | "deadline" | "task_join" | "transport_unavailable" | "invalid_path" | "communication" | "decode" | "request_size" | "response_size" | "identity" | "backend_rejected" | "backend_unknown" | "unrecognized_response" | "invalid_metadata";

export type ConfigFailureStage = "request" | "response" | "completion" | "reconcile" | "attach";

export type ConfigurationFailureDetails = { operation: ConfigurationFailureOperation; reason: ConfigurationFailureReason; };

export type ConfigurationFailureOperation = "source" | "preview" | "adopt" | "reconcile" | "publication";

export type ConfigurationFailureReason = "invalid_path" | "builtin_read_only" | "invalid_import" | "import_escape" | "already_exists" | "not_found" | "entry_conflict" | "entry_digest_mismatch" | "entry_path_mismatch" | "tree_digest_mismatch" | "path_changed_more_than_once" | "duplicate_path" | "duplicate_schema_provider" | "duplicate_schema_namespace" | "invalid_schema_contribution" | "limit_exceeded" | "catalog_changed" | "editing_head_changed" | "legacy_export_changed" | "source_content_changed" | "owner_required" | "workspace_inactive" | "workspace_draining" | "request_identity_conflict" | "publication_dispatch_unavailable" | "publication_projection_unavailable" | "publication_registry_unavailable";

export type DriveApiErrorCode = "denied" | "not_found" | "conflict" | "invalid" | "limit" | "storage_unavailable" | "outcome_unknown";

export type DriveClientFailureDetails = { operation: DriveClientFailureOperation; reason: DriveClientFailureReason; server?: DriveHttpFailureDetails | null; };

export type DriveClientFailureOperation = "workspace" | "capabilities" | "root" | "metadata" | "list" | "search" | "read" | "bytes" | "create_folder" | "create_text" | "update_text" | "relocate" | "delete" | "upload" | "request_status";

export type DriveClientFailureReason = "authorization_denied" | "authorization_failed" | "denied" | "not_found" | "conflict" | "invalid" | "limit" | "unavailable" | "outcome_unknown";

export type DriveFailureClassification = "not_committed" | "unknown";

export type DriveFailureDetails = { operation: DriveFailureOperation; reason: DriveFailureReason; };

export type DriveFailureOperation = "open" | "attach_workspace_authority" | "check_authorized" | "root" | "metadata" | "list" | "request_status" | "create_folder" | "create_file" | "update" | "relocate" | "delete" | "read" | "read_text" | "search" | "collect" | "fence" | "purge";

export type DriveFailureReason = "invalid" | "not_found" | "conflict" | "denied" | "fenced" | "storage_failure" | "metadata_failure" | "blob_failure" | "encoding_failure";

export type DriveHttpFailureDetails = { classification: DriveFailureClassification; code: DriveApiErrorCode; operation: DriveHttpOperation; storage?: DriveFailureDetails | null; };

export type DriveHttpOperation = "request" | "root" | "capabilities" | "metadata" | "list" | "search" | "read_text" | "read_chunk" | "download" | "mutate" | "upload" | "request_status" | "grant_create" | "grant_revoke" | "grant_list";

export type RepositoryAccessDetails = { operation: RepositoryAccessOperation; reason: RepositoryAccessReason; };

export type RepositoryAccessOperation = "project_access" | "create_credential" | "rotate_credential" | "delete_credential" | "put_host_trust" | "delete_host_trust" | "default_ssh_binding" | "lease_ssh_access";

export type RepositoryAccessReason = "plain_http_source_unsupported" | "unknown_repository" | "ssh_source_required" | "binding_credential_not_found" | "credential_inactive" | "binding_host_trust_not_found" | "host_endpoint_mismatch" | "credential_already_exists" | "workspace_default_credential_immutable" | "credential_not_found" | "credential_observation_changed" | "credential_key_changed" | "credential_referenced" | "credential_retained" | "host_trust_observation_changed" | "host_trust_not_found" | "host_trust_key_changed" | "host_trust_referenced" | "ambiguous_host_trust" | "credential_unavailable_or_inactive" | "host_trust_unavailable";

export type RepositoryFailureDetails = { operation: RepositoryFailureOperation; reason: RepositoryFailureReason; };

export type RepositoryFailureOperation = "register" | "summary" | "summary_by_key" | "recent_log" | "observe_merge_target" | "observe_commit" | "ensure_ancestor";

export type RepositoryFailureReason = "registration_intent_conflict" | "unknown_repository" | "unsupported_provider" | "missing_default_selector" | "invalid_selector" | "commit_not_found" | "invalid_commit_relation" | "provider_failure";

export type WorkdirDenialReason = "scope_resolution_unavailable" | "scope_comparison_unavailable" | "scoped_capability_denied" | "logical_scope_exceeded" | "invalid_scoped_path" | "child_write_lease_conflict" | "empty_scope" | "parent_capability_denied" | "command_requires_writable_scope" | "parent_scope_exceeded" | "cwd_outside_readable_scope" | "read_only_session" | "external_root_symlink" | "external_root_changed" | "external_root_moved" | "external_scope_symlink" | "attachment_scope_exceeded" | "delegated_scope_exceeded" | "checkout_output_root_exceeded" | "provider_root_exceeded" | "provider_symlink_denied" | "checkout_target_denied" | "checkout_create_parent_denied" | "checkout_search_path_denied" | "checkout_enumeration_denied" | "path_out_of_scope" | "symlink_target_out_of_scope" | "path_read_only" | "os_permission_denied" | "unclassified_permission_denied";

export type WorkdirFailureDetails = { classification: WorkdirTransportErrorCode; denial_reason?: WorkdirDenialReason | null; failure_reason?: WorkdirFailureReason | null; operation: WorkdirFailureOperation; };

export type WorkdirFailureOperation = "session_open" | "session_close" | "attachment_projection" | "authorize_scope" | "scope_rules_overlap" | "checkout_search" | "checkout_observe" | "checkout_execute" | "stat" | "read" | "read_bytes" | "write" | "edit" | "list" | "glob" | "grep" | "command_start" | "command_status" | "command_output" | "command_cancel";

export type WorkdirFailureReason = "session_closed" | "provider_unavailable" | "provider_grant_expired" | "provider_admission_closed" | "provider_dispatch_rejected" | "provider_response_lost" | "provider_operation_timeout" | "command_terminal_missing" | "audit_store_unavailable" | "http_timeout" | "http_connect" | "http_transfer" | "http_response_invalid" | "http_response_too_large" | "operation_result_mismatch" | "read_limit_exceeded" | "session_identity_mismatch" | "session_open_task_failed" | "session_open_failed" | "authorization_missing" | "authorization_stale" | "authorization_unavailable" | "signing_identity_unavailable" | "authorization_sign_failed" | "runtime_stopped" | "runtime_state_unavailable" | "runtime_backend_unavailable" | "runtime_owner_mismatch" | "runtime_invalid_request" | "working_directory_not_found" | "working_directory_not_active" | "working_directory_record_invalid" | "working_directory_access_unavailable" | "session_registry_unavailable" | "attachment_projection_unavailable";

export type WorkdirManagementFailure = { operation: WorkdirManagementOperation; reason: WorkdirManagementReason; };

export type WorkdirManagementOperation = "create" | "list" | "detail" | "cleanup" | "repository_access" | "repository_ref" | "attach" | "detach" | "grant_create" | "grant_inspect" | "grant_revoke" | "grant_connect" | "grant_cleanup";

export type WorkdirManagementReason = "embedded_worker_workdir_unsupported" | "external_workdir_legacy_permissions" | "workdir_attachment_registry_not_found" | "workdir_attachment_not_available" | "workdir_attachment_grant_bound_required" | "workdir_external_grant_unavailable" | "external_workdir_grant_state_outcome_unknown" | "external_workdir_grant_not_found" | "external_workdir_grant_creator_required" | "external_workdir_grant_display_name_invalid" | "external_workdir_grant_ttl_invalid" | "external_workdir_grant_provider_identity_invalid" | "external_workdir_grant_revoked" | "external_workdir_grant_expired" | "external_workdir_grant_activation_failed" | "external_workdir_connection_state_failed" | "external_workdir_connection_transport_failed" | "external_workdir_connection_protocol_failed" | "external_workdir_connection_persistence_failed" | "external_workdir_connection_query_failed" | "external_workdir_grant_audit_failed" | "external_workdir_grant_cleanup_persistence_failed" | "external_workdir_grant_cleanup_query_failed" | "external_workdir_grant_attachment_release_failed" | "external_workdir_grant_cleanup_outcome_unknown" | "workdir_attachment_conflict" | "workdir_attachment_persistence_failed" | "workdir_attachment_state_outcome_unknown" | "workdir_attachment_projection_failed" | "workdir_attachment_runtime_result_unavailable" | "workdir_occupied" | "working_directory_not_found" | "working_directory_not_active" | "working_directory_create_failed" | "working_directory_exists" | "working_directory_git_failed" | "working_directory_git_unavailable" | "working_directory_id_invalid" | "working_directory_list_failed" | "working_directory_materializer_unsupported" | "working_directory_record_invalid" | "working_directory_record_serialize_failed" | "working_directory_record_write_failed" | "working_directory_relative_cwd_escape_rejected" | "working_directory_relative_cwd_invalid" | "working_directory_relative_cwd_unavailable" | "working_directory_non_utf8_path" | "working_directory_root_unavailable" | "working_directory_remote_repository_access_invalid" | "working_directory_remote_repository_access_required" | "working_directory_remote_repository_host_trust_mismatch" | "working_directory_repository_access_binding_mismatch" | "working_directory_repository_access_expired" | "working_directory_repository_access_not_applicable" | "working_directory_repository_access_setup_failed" | "working_directory_repository_access_unavailable" | "working_directory_repository_agent_failed" | "working_directory_repository_agent_unavailable" | "working_directory_repository_limit_exceeded" | "working_directory_repository_materialization_authority_invalid" | "working_directory_repository_materialization_authority_required" | "working_directory_repository_plain_http_unsupported" | "working_directory_repository_provider_unsupported" | "working_directory_repository_selector_invalid" | "working_directory_repository_selector_unresolved" | "working_directory_repository_source_invalid" | "working_directory_repository_timeout" | "working_directory_repository_clone_failed" | "working_directory_repository_checkout_failed" | "working_directory_repository_policy_failed" | "working_directory_repository_access_denied" | "working_directory_repository_host_trust_failed" | "working_directory_repository_authentication_failed" | "working_directory_repository_connection_failed" | "working_directory_repository_not_found" | "repository_ref_not_found" | "repository_ref_provider_timeout" | "repository_ref_provider_unavailable" | "repository_ref_provider_auth_failed" | "repository_ref_response_invalid" | "repository_ref_selector_invalid" | "working_directory_cleanup_mount_present" | "working_directory_cleanup_mount_check_unavailable" | "working_directory_cleanup_permission_denied" | "working_directory_cleanup_resource_busy" | "working_directory_cleanup_storage_unavailable" | "working_directory_cleanup_ownership_unknown" | "working_directory_cleanup_changes_present" | "working_directory_cleanup_changes_unknown";

export type WorkdirTransportErrorCode = "not_found" | "conflict" | "outcome_unknown" | "not_dispatched" | "unsupported" | "invalid_request" | "denied" | "out_of_scope" | "symlink_out_of_scope" | "broken_symlink" | "symlink_target_is_directory" | "read_only" | "is_directory" | "symlink_directory_not_traversed" | "unknown_command" | "unavailable" | "io" | "transport" | "internal";

export type DomainFailureDetails = ConfigFailureDetails | ConfigurationFailureDetails | DriveClientFailureDetails | DriveFailureDetails | DriveHttpFailureDetails | RepositoryAccessDetails | RepositoryFailureDetails | WorkdirFailureDetails | WorkdirManagementFailure;

export const DOMAIN_FAILURE_CONTRACTS = {
  "configuration": {
    "categories": {
      "already_exists": "invalid_input",
      "builtin_read_only": "invalid_input",
      "catalog_changed": "conflict",
      "duplicate_path": "invalid_input",
      "duplicate_schema_namespace": "internal",
      "duplicate_schema_provider": "internal",
      "editing_head_changed": "conflict",
      "entry_conflict": "conflict",
      "entry_digest_mismatch": "invalid_input",
      "entry_path_mismatch": "invalid_input",
      "import_escape": "invalid_input",
      "invalid_import": "invalid_input",
      "invalid_path": "invalid_input",
      "invalid_schema_contribution": "internal",
      "legacy_export_changed": "conflict",
      "limit_exceeded": "invalid_input",
      "not_found": "invalid_input",
      "owner_required": "authorization",
      "path_changed_more_than_once": "invalid_input",
      "publication_dispatch_unavailable": "internal",
      "publication_projection_unavailable": "internal",
      "publication_registry_unavailable": "internal",
      "request_identity_conflict": "conflict",
      "source_content_changed": "conflict",
      "tree_digest_mismatch": "invalid_input",
      "workspace_draining": "conflict",
      "workspace_inactive": "conflict"
    },
    "category_field": "reason",
    "fields": {
      "operation": [
        "source",
        "preview",
        "adopt",
        "reconcile",
        "publication"
      ],
      "reason": [
        "invalid_path",
        "builtin_read_only",
        "invalid_import",
        "import_escape",
        "already_exists",
        "not_found",
        "entry_conflict",
        "entry_digest_mismatch",
        "entry_path_mismatch",
        "tree_digest_mismatch",
        "path_changed_more_than_once",
        "duplicate_path",
        "duplicate_schema_provider",
        "duplicate_schema_namespace",
        "invalid_schema_contribution",
        "limit_exceeded",
        "catalog_changed",
        "editing_head_changed",
        "legacy_export_changed",
        "source_content_changed",
        "owner_required",
        "workspace_inactive",
        "workspace_draining",
        "request_identity_conflict",
        "publication_dispatch_unavailable",
        "publication_projection_unavailable",
        "publication_registry_unavailable"
      ]
    },
    "reason_fields": [
      "reason"
    ],
    "reasons": [
      "already_exists",
      "builtin_read_only",
      "catalog_changed",
      "duplicate_path",
      "duplicate_schema_namespace",
      "duplicate_schema_provider",
      "editing_head_changed",
      "entry_conflict",
      "entry_digest_mismatch",
      "entry_path_mismatch",
      "import_escape",
      "invalid_import",
      "invalid_path",
      "invalid_schema_contribution",
      "legacy_export_changed",
      "limit_exceeded",
      "not_found",
      "owner_required",
      "path_changed_more_than_once",
      "publication_dispatch_unavailable",
      "publication_projection_unavailable",
      "publication_registry_unavailable",
      "request_identity_conflict",
      "source_content_changed",
      "tree_digest_mismatch",
      "workspace_draining",
      "workspace_inactive"
    ],
    "required": [
      "operation",
      "reason"
    ],
    "version": 1
  },
  "drive": {
    "categories": {
      "blob_failure": "internal",
      "conflict": "conflict",
      "denied": "authorization",
      "encoding_failure": "internal",
      "fenced": "conflict",
      "invalid": "invalid_input",
      "metadata_failure": "internal",
      "not_found": "invalid_input",
      "storage_failure": "internal"
    },
    "category_field": "reason",
    "fields": {
      "operation": [
        "open",
        "attach_workspace_authority",
        "check_authorized",
        "root",
        "metadata",
        "list",
        "request_status",
        "create_folder",
        "create_file",
        "update",
        "relocate",
        "delete",
        "read",
        "read_text",
        "search",
        "collect",
        "fence",
        "purge"
      ],
      "reason": [
        "invalid",
        "not_found",
        "conflict",
        "denied",
        "fenced",
        "storage_failure",
        "metadata_failure",
        "blob_failure",
        "encoding_failure"
      ]
    },
    "reason_fields": [
      "reason"
    ],
    "reasons": [
      "blob_failure",
      "conflict",
      "denied",
      "encoding_failure",
      "fenced",
      "invalid",
      "metadata_failure",
      "not_found",
      "storage_failure"
    ],
    "required": [
      "operation",
      "reason"
    ],
    "version": 1
  },
  "drive_http": {
    "categories": {
      "conflict": "conflict",
      "denied": "authorization",
      "invalid": "invalid_input",
      "limit": "invalid_input",
      "not_found": "invalid_input",
      "outcome_unknown": "internal",
      "storage_unavailable": "internal"
    },
    "category_field": "code",
    "fields": {
      "classification": [
        "not_committed",
        "unknown"
      ],
      "code": [
        "denied",
        "not_found",
        "conflict",
        "invalid",
        "limit",
        "storage_unavailable",
        "outcome_unknown"
      ],
      "operation": [
        "request",
        "root",
        "capabilities",
        "metadata",
        "list",
        "search",
        "read_text",
        "read_chunk",
        "download",
        "mutate",
        "upload",
        "request_status",
        "grant_create",
        "grant_revoke",
        "grant_list"
      ],
      "storage": {
        "any_of": [
          {
            "fields": {
              "operation": [
                "open",
                "attach_workspace_authority",
                "check_authorized",
                "root",
                "metadata",
                "list",
                "request_status",
                "create_folder",
                "create_file",
                "update",
                "relocate",
                "delete",
                "read",
                "read_text",
                "search",
                "collect",
                "fence",
                "purge"
              ],
              "reason": [
                "invalid",
                "not_found",
                "conflict",
                "denied",
                "fenced",
                "storage_failure",
                "metadata_failure",
                "blob_failure",
                "encoding_failure"
              ]
            },
            "required": [
              "operation",
              "reason"
            ]
          },
          [
            null
          ]
        ]
      }
    },
    "reason_fields": [
      "code"
    ],
    "reasons": [
      "conflict",
      "denied",
      "invalid",
      "limit",
      "not_found",
      "outcome_unknown",
      "storage_unavailable"
    ],
    "required": [
      "operation",
      "code",
      "classification"
    ],
    "version": 1
  },
  "repository": {
    "categories": {
      "commit_not_found": "invalid_input",
      "invalid_commit_relation": "invalid_input",
      "invalid_selector": "invalid_input",
      "missing_default_selector": "invalid_input",
      "provider_failure": "dependency",
      "registration_intent_conflict": "conflict",
      "unknown_repository": "invalid_input",
      "unsupported_provider": "dependency"
    },
    "category_field": "reason",
    "fields": {
      "operation": [
        "register",
        "summary",
        "summary_by_key",
        "recent_log",
        "observe_merge_target",
        "observe_commit",
        "ensure_ancestor"
      ],
      "reason": [
        "registration_intent_conflict",
        "unknown_repository",
        "unsupported_provider",
        "missing_default_selector",
        "invalid_selector",
        "commit_not_found",
        "invalid_commit_relation",
        "provider_failure"
      ]
    },
    "reason_fields": [
      "reason"
    ],
    "reasons": [
      "commit_not_found",
      "invalid_commit_relation",
      "invalid_selector",
      "missing_default_selector",
      "provider_failure",
      "registration_intent_conflict",
      "unknown_repository",
      "unsupported_provider"
    ],
    "required": [
      "operation",
      "reason"
    ],
    "version": 1
  },
  "repository_access": {
    "categories": {
      "ambiguous_host_trust": "invalid_input",
      "binding_credential_not_found": "invalid_input",
      "binding_host_trust_not_found": "invalid_input",
      "credential_already_exists": "conflict",
      "credential_inactive": "invalid_input",
      "credential_key_changed": "conflict",
      "credential_not_found": "invalid_input",
      "credential_observation_changed": "conflict",
      "credential_referenced": "conflict",
      "credential_retained": "conflict",
      "credential_unavailable_or_inactive": "invalid_input",
      "host_endpoint_mismatch": "invalid_input",
      "host_trust_key_changed": "conflict",
      "host_trust_not_found": "invalid_input",
      "host_trust_observation_changed": "conflict",
      "host_trust_referenced": "conflict",
      "host_trust_unavailable": "invalid_input",
      "plain_http_source_unsupported": "invalid_input",
      "ssh_source_required": "invalid_input",
      "unknown_repository": "invalid_input",
      "workspace_default_credential_immutable": "conflict"
    },
    "category_field": "reason",
    "fields": {
      "operation": [
        "project_access",
        "create_credential",
        "rotate_credential",
        "delete_credential",
        "put_host_trust",
        "delete_host_trust",
        "default_ssh_binding",
        "lease_ssh_access"
      ],
      "reason": [
        "plain_http_source_unsupported",
        "unknown_repository",
        "ssh_source_required",
        "binding_credential_not_found",
        "credential_inactive",
        "binding_host_trust_not_found",
        "host_endpoint_mismatch",
        "credential_already_exists",
        "workspace_default_credential_immutable",
        "credential_not_found",
        "credential_observation_changed",
        "credential_key_changed",
        "credential_referenced",
        "credential_retained",
        "host_trust_observation_changed",
        "host_trust_not_found",
        "host_trust_key_changed",
        "host_trust_referenced",
        "ambiguous_host_trust",
        "credential_unavailable_or_inactive",
        "host_trust_unavailable"
      ]
    },
    "reason_fields": [
      "reason"
    ],
    "reasons": [
      "ambiguous_host_trust",
      "binding_credential_not_found",
      "binding_host_trust_not_found",
      "credential_already_exists",
      "credential_inactive",
      "credential_key_changed",
      "credential_not_found",
      "credential_observation_changed",
      "credential_referenced",
      "credential_retained",
      "credential_unavailable_or_inactive",
      "host_endpoint_mismatch",
      "host_trust_key_changed",
      "host_trust_not_found",
      "host_trust_observation_changed",
      "host_trust_referenced",
      "host_trust_unavailable",
      "plain_http_source_unsupported",
      "ssh_source_required",
      "unknown_repository",
      "workspace_default_credential_immutable"
    ],
    "required": [
      "operation",
      "reason"
    ],
    "version": 1
  },
  "workdir": {
    "categories": {
      "broken_symlink": "invalid_input",
      "conflict": "conflict",
      "denied": "authorization",
      "internal": "internal",
      "invalid_request": "invalid_input",
      "io": "internal",
      "is_directory": "invalid_input",
      "not_dispatched": "dependency",
      "not_found": "invalid_input",
      "out_of_scope": "authorization",
      "outcome_unknown": "unknown",
      "read_only": "authorization",
      "symlink_directory_not_traversed": "invalid_input",
      "symlink_out_of_scope": "authorization",
      "symlink_target_is_directory": "invalid_input",
      "transport": "dependency",
      "unavailable": "dependency",
      "unknown_command": "invalid_input",
      "unsupported": "invalid_input"
    },
    "category_field": "classification",
    "fields": {
      "classification": [
        "not_found",
        "conflict",
        "outcome_unknown",
        "not_dispatched",
        "unsupported",
        "invalid_request",
        "denied",
        "out_of_scope",
        "symlink_out_of_scope",
        "broken_symlink",
        "symlink_target_is_directory",
        "read_only",
        "is_directory",
        "symlink_directory_not_traversed",
        "unknown_command",
        "unavailable",
        "io",
        "transport",
        "internal"
      ],
      "denial_reason": [
        "scope_resolution_unavailable",
        "scope_comparison_unavailable",
        "scoped_capability_denied",
        "logical_scope_exceeded",
        "invalid_scoped_path",
        "child_write_lease_conflict",
        "empty_scope",
        "parent_capability_denied",
        "command_requires_writable_scope",
        "parent_scope_exceeded",
        "cwd_outside_readable_scope",
        "read_only_session",
        "external_root_symlink",
        "external_root_changed",
        "external_root_moved",
        "external_scope_symlink",
        "attachment_scope_exceeded",
        "delegated_scope_exceeded",
        "checkout_output_root_exceeded",
        "provider_root_exceeded",
        "provider_symlink_denied",
        "checkout_target_denied",
        "checkout_create_parent_denied",
        "checkout_search_path_denied",
        "checkout_enumeration_denied",
        "path_out_of_scope",
        "symlink_target_out_of_scope",
        "path_read_only",
        "os_permission_denied",
        "unclassified_permission_denied",
        null
      ],
      "failure_reason": [
        "session_closed",
        "provider_unavailable",
        "provider_grant_expired",
        "provider_admission_closed",
        "provider_dispatch_rejected",
        "provider_response_lost",
        "provider_operation_timeout",
        "command_terminal_missing",
        "audit_store_unavailable",
        "http_timeout",
        "http_connect",
        "http_transfer",
        "http_response_invalid",
        "http_response_too_large",
        "operation_result_mismatch",
        "read_limit_exceeded",
        "session_identity_mismatch",
        "session_open_task_failed",
        "session_open_failed",
        "authorization_missing",
        "authorization_stale",
        "authorization_unavailable",
        "signing_identity_unavailable",
        "authorization_sign_failed",
        "runtime_stopped",
        "runtime_state_unavailable",
        "runtime_backend_unavailable",
        "runtime_owner_mismatch",
        "runtime_invalid_request",
        "working_directory_not_found",
        "working_directory_not_active",
        "working_directory_record_invalid",
        "working_directory_access_unavailable",
        "session_registry_unavailable",
        "attachment_projection_unavailable",
        null
      ],
      "operation": [
        "session_open",
        "session_close",
        "attachment_projection",
        "authorize_scope",
        "scope_rules_overlap",
        "checkout_search",
        "checkout_observe",
        "checkout_execute",
        "stat",
        "read",
        "read_bytes",
        "write",
        "edit",
        "list",
        "glob",
        "grep",
        "command_start",
        "command_status",
        "command_output",
        "command_cancel"
      ]
    },
    "reason_fields": [
      "failure_reason",
      "denial_reason",
      "classification"
    ],
    "reasons": [
      "attachment_projection_unavailable",
      "attachment_scope_exceeded",
      "audit_store_unavailable",
      "authorization_missing",
      "authorization_sign_failed",
      "authorization_stale",
      "authorization_unavailable",
      "broken_symlink",
      "checkout_create_parent_denied",
      "checkout_enumeration_denied",
      "checkout_output_root_exceeded",
      "checkout_search_path_denied",
      "checkout_target_denied",
      "child_write_lease_conflict",
      "command_requires_writable_scope",
      "command_terminal_missing",
      "conflict",
      "cwd_outside_readable_scope",
      "delegated_scope_exceeded",
      "denied",
      "empty_scope",
      "external_root_changed",
      "external_root_moved",
      "external_root_symlink",
      "external_scope_symlink",
      "http_connect",
      "http_response_invalid",
      "http_response_too_large",
      "http_timeout",
      "http_transfer",
      "internal",
      "invalid_request",
      "invalid_scoped_path",
      "io",
      "is_directory",
      "logical_scope_exceeded",
      "not_dispatched",
      "not_found",
      "operation_result_mismatch",
      "os_permission_denied",
      "out_of_scope",
      "outcome_unknown",
      "parent_capability_denied",
      "parent_scope_exceeded",
      "path_out_of_scope",
      "path_read_only",
      "provider_admission_closed",
      "provider_dispatch_rejected",
      "provider_grant_expired",
      "provider_operation_timeout",
      "provider_response_lost",
      "provider_root_exceeded",
      "provider_symlink_denied",
      "provider_unavailable",
      "read_limit_exceeded",
      "read_only",
      "read_only_session",
      "runtime_backend_unavailable",
      "runtime_invalid_request",
      "runtime_owner_mismatch",
      "runtime_state_unavailable",
      "runtime_stopped",
      "scope_comparison_unavailable",
      "scope_resolution_unavailable",
      "scoped_capability_denied",
      "session_closed",
      "session_identity_mismatch",
      "session_open_failed",
      "session_open_task_failed",
      "session_registry_unavailable",
      "signing_identity_unavailable",
      "symlink_directory_not_traversed",
      "symlink_out_of_scope",
      "symlink_target_is_directory",
      "symlink_target_out_of_scope",
      "transport",
      "unavailable",
      "unclassified_permission_denied",
      "unknown_command",
      "unsupported",
      "working_directory_access_unavailable",
      "working_directory_not_active",
      "working_directory_not_found",
      "working_directory_record_invalid"
    ],
    "required": [
      "operation",
      "classification"
    ],
    "version": 1
  },
  "workdir_management": {
    "categories": {
      "embedded_worker_workdir_unsupported": "invalid_input",
      "external_workdir_connection_persistence_failed": "dependency",
      "external_workdir_connection_protocol_failed": "dependency",
      "external_workdir_connection_query_failed": "dependency",
      "external_workdir_connection_state_failed": "dependency",
      "external_workdir_connection_transport_failed": "dependency",
      "external_workdir_grant_activation_failed": "dependency",
      "external_workdir_grant_attachment_release_failed": "dependency",
      "external_workdir_grant_audit_failed": "dependency",
      "external_workdir_grant_cleanup_outcome_unknown": "unknown",
      "external_workdir_grant_cleanup_persistence_failed": "dependency",
      "external_workdir_grant_cleanup_query_failed": "dependency",
      "external_workdir_grant_creator_required": "authorization",
      "external_workdir_grant_display_name_invalid": "invalid_input",
      "external_workdir_grant_expired": "conflict",
      "external_workdir_grant_not_found": "invalid_input",
      "external_workdir_grant_provider_identity_invalid": "invalid_input",
      "external_workdir_grant_revoked": "conflict",
      "external_workdir_grant_state_outcome_unknown": "unknown",
      "external_workdir_grant_ttl_invalid": "invalid_input",
      "external_workdir_legacy_permissions": "authorization",
      "repository_ref_not_found": "invalid_input",
      "repository_ref_provider_auth_failed": "authorization",
      "repository_ref_provider_timeout": "dependency",
      "repository_ref_provider_unavailable": "dependency",
      "repository_ref_response_invalid": "dependency",
      "repository_ref_selector_invalid": "invalid_input",
      "workdir_attachment_conflict": "conflict",
      "workdir_attachment_grant_bound_required": "authorization",
      "workdir_attachment_not_available": "conflict",
      "workdir_attachment_persistence_failed": "dependency",
      "workdir_attachment_projection_failed": "dependency",
      "workdir_attachment_registry_not_found": "invalid_input",
      "workdir_attachment_runtime_result_unavailable": "unknown",
      "workdir_attachment_state_outcome_unknown": "unknown",
      "workdir_external_grant_unavailable": "dependency",
      "workdir_occupied": "conflict",
      "working_directory_cleanup_changes_present": "conflict",
      "working_directory_cleanup_changes_unknown": "conflict",
      "working_directory_cleanup_mount_check_unavailable": "dependency",
      "working_directory_cleanup_mount_present": "conflict",
      "working_directory_cleanup_ownership_unknown": "conflict",
      "working_directory_cleanup_permission_denied": "authorization",
      "working_directory_cleanup_resource_busy": "conflict",
      "working_directory_cleanup_storage_unavailable": "dependency",
      "working_directory_create_failed": "dependency",
      "working_directory_exists": "conflict",
      "working_directory_git_failed": "dependency",
      "working_directory_git_unavailable": "dependency",
      "working_directory_id_invalid": "invalid_input",
      "working_directory_list_failed": "dependency",
      "working_directory_materializer_unsupported": "invalid_input",
      "working_directory_non_utf8_path": "invalid_input",
      "working_directory_not_active": "conflict",
      "working_directory_not_found": "invalid_input",
      "working_directory_record_invalid": "conflict",
      "working_directory_record_serialize_failed": "internal",
      "working_directory_record_write_failed": "dependency",
      "working_directory_relative_cwd_escape_rejected": "authorization",
      "working_directory_relative_cwd_invalid": "invalid_input",
      "working_directory_relative_cwd_unavailable": "invalid_input",
      "working_directory_remote_repository_access_invalid": "authorization",
      "working_directory_remote_repository_access_required": "authorization",
      "working_directory_remote_repository_host_trust_mismatch": "authorization",
      "working_directory_repository_access_binding_mismatch": "authorization",
      "working_directory_repository_access_denied": "authorization",
      "working_directory_repository_access_expired": "authorization",
      "working_directory_repository_access_not_applicable": "invalid_input",
      "working_directory_repository_access_setup_failed": "dependency",
      "working_directory_repository_access_unavailable": "dependency",
      "working_directory_repository_agent_failed": "dependency",
      "working_directory_repository_agent_unavailable": "dependency",
      "working_directory_repository_authentication_failed": "authorization",
      "working_directory_repository_checkout_failed": "dependency",
      "working_directory_repository_clone_failed": "dependency",
      "working_directory_repository_connection_failed": "dependency",
      "working_directory_repository_host_trust_failed": "authorization",
      "working_directory_repository_limit_exceeded": "invalid_input",
      "working_directory_repository_materialization_authority_invalid": "authorization",
      "working_directory_repository_materialization_authority_required": "authorization",
      "working_directory_repository_not_found": "invalid_input",
      "working_directory_repository_plain_http_unsupported": "invalid_input",
      "working_directory_repository_policy_failed": "dependency",
      "working_directory_repository_provider_unsupported": "invalid_input",
      "working_directory_repository_selector_invalid": "invalid_input",
      "working_directory_repository_selector_unresolved": "invalid_input",
      "working_directory_repository_source_invalid": "invalid_input",
      "working_directory_repository_timeout": "dependency",
      "working_directory_root_unavailable": "dependency"
    },
    "category_field": "reason",
    "fields": {
      "operation": [
        "create",
        "list",
        "detail",
        "cleanup",
        "repository_access",
        "repository_ref",
        "attach",
        "detach",
        "grant_create",
        "grant_inspect",
        "grant_revoke",
        "grant_connect",
        "grant_cleanup"
      ],
      "reason": [
        "embedded_worker_workdir_unsupported",
        "external_workdir_legacy_permissions",
        "workdir_attachment_registry_not_found",
        "workdir_attachment_not_available",
        "workdir_attachment_grant_bound_required",
        "workdir_external_grant_unavailable",
        "external_workdir_grant_state_outcome_unknown",
        "external_workdir_grant_not_found",
        "external_workdir_grant_creator_required",
        "external_workdir_grant_display_name_invalid",
        "external_workdir_grant_ttl_invalid",
        "external_workdir_grant_provider_identity_invalid",
        "external_workdir_grant_revoked",
        "external_workdir_grant_expired",
        "external_workdir_grant_activation_failed",
        "external_workdir_connection_state_failed",
        "external_workdir_connection_transport_failed",
        "external_workdir_connection_protocol_failed",
        "external_workdir_connection_persistence_failed",
        "external_workdir_connection_query_failed",
        "external_workdir_grant_audit_failed",
        "external_workdir_grant_cleanup_persistence_failed",
        "external_workdir_grant_cleanup_query_failed",
        "external_workdir_grant_attachment_release_failed",
        "external_workdir_grant_cleanup_outcome_unknown",
        "workdir_attachment_conflict",
        "workdir_attachment_persistence_failed",
        "workdir_attachment_state_outcome_unknown",
        "workdir_attachment_projection_failed",
        "workdir_attachment_runtime_result_unavailable",
        "workdir_occupied",
        "working_directory_not_found",
        "working_directory_not_active",
        "working_directory_create_failed",
        "working_directory_exists",
        "working_directory_git_failed",
        "working_directory_git_unavailable",
        "working_directory_id_invalid",
        "working_directory_list_failed",
        "working_directory_materializer_unsupported",
        "working_directory_record_invalid",
        "working_directory_record_serialize_failed",
        "working_directory_record_write_failed",
        "working_directory_relative_cwd_escape_rejected",
        "working_directory_relative_cwd_invalid",
        "working_directory_relative_cwd_unavailable",
        "working_directory_non_utf8_path",
        "working_directory_root_unavailable",
        "working_directory_remote_repository_access_invalid",
        "working_directory_remote_repository_access_required",
        "working_directory_remote_repository_host_trust_mismatch",
        "working_directory_repository_access_binding_mismatch",
        "working_directory_repository_access_expired",
        "working_directory_repository_access_not_applicable",
        "working_directory_repository_access_setup_failed",
        "working_directory_repository_access_unavailable",
        "working_directory_repository_agent_failed",
        "working_directory_repository_agent_unavailable",
        "working_directory_repository_limit_exceeded",
        "working_directory_repository_materialization_authority_invalid",
        "working_directory_repository_materialization_authority_required",
        "working_directory_repository_plain_http_unsupported",
        "working_directory_repository_provider_unsupported",
        "working_directory_repository_selector_invalid",
        "working_directory_repository_selector_unresolved",
        "working_directory_repository_source_invalid",
        "working_directory_repository_timeout",
        "working_directory_repository_clone_failed",
        "working_directory_repository_checkout_failed",
        "working_directory_repository_policy_failed",
        "working_directory_repository_access_denied",
        "working_directory_repository_host_trust_failed",
        "working_directory_repository_authentication_failed",
        "working_directory_repository_connection_failed",
        "working_directory_repository_not_found",
        "repository_ref_not_found",
        "repository_ref_provider_timeout",
        "repository_ref_provider_unavailable",
        "repository_ref_provider_auth_failed",
        "repository_ref_response_invalid",
        "repository_ref_selector_invalid",
        "working_directory_cleanup_mount_present",
        "working_directory_cleanup_mount_check_unavailable",
        "working_directory_cleanup_permission_denied",
        "working_directory_cleanup_resource_busy",
        "working_directory_cleanup_storage_unavailable",
        "working_directory_cleanup_ownership_unknown",
        "working_directory_cleanup_changes_present",
        "working_directory_cleanup_changes_unknown"
      ]
    },
    "reason_fields": [
      "reason"
    ],
    "reasons": [
      "embedded_worker_workdir_unsupported",
      "external_workdir_connection_persistence_failed",
      "external_workdir_connection_protocol_failed",
      "external_workdir_connection_query_failed",
      "external_workdir_connection_state_failed",
      "external_workdir_connection_transport_failed",
      "external_workdir_grant_activation_failed",
      "external_workdir_grant_attachment_release_failed",
      "external_workdir_grant_audit_failed",
      "external_workdir_grant_cleanup_outcome_unknown",
      "external_workdir_grant_cleanup_persistence_failed",
      "external_workdir_grant_cleanup_query_failed",
      "external_workdir_grant_creator_required",
      "external_workdir_grant_display_name_invalid",
      "external_workdir_grant_expired",
      "external_workdir_grant_not_found",
      "external_workdir_grant_provider_identity_invalid",
      "external_workdir_grant_revoked",
      "external_workdir_grant_state_outcome_unknown",
      "external_workdir_grant_ttl_invalid",
      "external_workdir_legacy_permissions",
      "repository_ref_not_found",
      "repository_ref_provider_auth_failed",
      "repository_ref_provider_timeout",
      "repository_ref_provider_unavailable",
      "repository_ref_response_invalid",
      "repository_ref_selector_invalid",
      "workdir_attachment_conflict",
      "workdir_attachment_grant_bound_required",
      "workdir_attachment_not_available",
      "workdir_attachment_persistence_failed",
      "workdir_attachment_projection_failed",
      "workdir_attachment_registry_not_found",
      "workdir_attachment_runtime_result_unavailable",
      "workdir_attachment_state_outcome_unknown",
      "workdir_external_grant_unavailable",
      "workdir_occupied",
      "working_directory_cleanup_changes_present",
      "working_directory_cleanup_changes_unknown",
      "working_directory_cleanup_mount_check_unavailable",
      "working_directory_cleanup_mount_present",
      "working_directory_cleanup_ownership_unknown",
      "working_directory_cleanup_permission_denied",
      "working_directory_cleanup_resource_busy",
      "working_directory_cleanup_storage_unavailable",
      "working_directory_create_failed",
      "working_directory_exists",
      "working_directory_git_failed",
      "working_directory_git_unavailable",
      "working_directory_id_invalid",
      "working_directory_list_failed",
      "working_directory_materializer_unsupported",
      "working_directory_non_utf8_path",
      "working_directory_not_active",
      "working_directory_not_found",
      "working_directory_record_invalid",
      "working_directory_record_serialize_failed",
      "working_directory_record_write_failed",
      "working_directory_relative_cwd_escape_rejected",
      "working_directory_relative_cwd_invalid",
      "working_directory_relative_cwd_unavailable",
      "working_directory_remote_repository_access_invalid",
      "working_directory_remote_repository_access_required",
      "working_directory_remote_repository_host_trust_mismatch",
      "working_directory_repository_access_binding_mismatch",
      "working_directory_repository_access_denied",
      "working_directory_repository_access_expired",
      "working_directory_repository_access_not_applicable",
      "working_directory_repository_access_setup_failed",
      "working_directory_repository_access_unavailable",
      "working_directory_repository_agent_failed",
      "working_directory_repository_agent_unavailable",
      "working_directory_repository_authentication_failed",
      "working_directory_repository_checkout_failed",
      "working_directory_repository_clone_failed",
      "working_directory_repository_connection_failed",
      "working_directory_repository_host_trust_failed",
      "working_directory_repository_limit_exceeded",
      "working_directory_repository_materialization_authority_invalid",
      "working_directory_repository_materialization_authority_required",
      "working_directory_repository_not_found",
      "working_directory_repository_plain_http_unsupported",
      "working_directory_repository_policy_failed",
      "working_directory_repository_provider_unsupported",
      "working_directory_repository_selector_invalid",
      "working_directory_repository_selector_unresolved",
      "working_directory_repository_source_invalid",
      "working_directory_repository_timeout",
      "working_directory_root_unavailable"
    ],
    "required": [
      "operation",
      "reason"
    ],
    "version": 1
  },
  "workspace_config_client": {
    "categories": {
      "authorization_denied": "authorization",
      "authorization_failed": "dependency",
      "backend_rejected": "dependency",
      "backend_unknown": "unknown",
      "communication": "dependency",
      "deadline": "dependency",
      "decode": "internal",
      "identity": "authorization",
      "invalid_metadata": "dependency",
      "invalid_path": "invalid_input",
      "invalid_request": "invalid_input",
      "request_size": "invalid_input",
      "response_size": "dependency",
      "task_join": "internal",
      "transport_unavailable": "dependency",
      "unrecognized_response": "dependency"
    },
    "category_field": "reason",
    "fields": {
      "operation": [
        "current",
        "attach",
        "observe",
        "read",
        "commit",
        "post"
      ],
      "reason": [
        "authorization_denied",
        "authorization_failed",
        "invalid_request",
        "deadline",
        "task_join",
        "transport_unavailable",
        "invalid_path",
        "communication",
        "decode",
        "request_size",
        "response_size",
        "identity",
        "backend_rejected",
        "backend_unknown",
        "unrecognized_response",
        "invalid_metadata"
      ],
      "stage": [
        "request",
        "response",
        "completion",
        "reconcile",
        "attach"
      ]
    },
    "reason_fields": [
      "reason"
    ],
    "reasons": [
      "authorization_denied",
      "authorization_failed",
      "backend_rejected",
      "backend_unknown",
      "communication",
      "deadline",
      "decode",
      "identity",
      "invalid_metadata",
      "invalid_path",
      "invalid_request",
      "request_size",
      "response_size",
      "task_join",
      "transport_unavailable",
      "unrecognized_response"
    ],
    "required": [
      "operation",
      "stage",
      "reason"
    ],
    "version": 1
  },
  "workspace_drive_client": {
    "categories": {
      "authorization_denied": "authorization",
      "authorization_failed": "authorization",
      "conflict": "conflict",
      "denied": "authorization",
      "invalid": "invalid_input",
      "limit": "invalid_input",
      "not_found": "invalid_input",
      "outcome_unknown": "unknown",
      "unavailable": "dependency"
    },
    "category_field": "reason",
    "fields": {
      "operation": [
        "workspace",
        "capabilities",
        "root",
        "metadata",
        "list",
        "search",
        "read",
        "bytes",
        "create_folder",
        "create_text",
        "update_text",
        "relocate",
        "delete",
        "upload",
        "request_status"
      ],
      "reason": [
        "authorization_denied",
        "authorization_failed",
        "denied",
        "not_found",
        "conflict",
        "invalid",
        "limit",
        "unavailable",
        "outcome_unknown"
      ],
      "server": {
        "any_of": [
          {
            "fields": {
              "classification": [
                "not_committed",
                "unknown"
              ],
              "code": [
                "denied",
                "not_found",
                "conflict",
                "invalid",
                "limit",
                "storage_unavailable",
                "outcome_unknown"
              ],
              "operation": [
                "request",
                "root",
                "capabilities",
                "metadata",
                "list",
                "search",
                "read_text",
                "read_chunk",
                "download",
                "mutate",
                "upload",
                "request_status",
                "grant_create",
                "grant_revoke",
                "grant_list"
              ],
              "storage": {
                "any_of": [
                  {
                    "fields": {
                      "operation": [
                        "open",
                        "attach_workspace_authority",
                        "check_authorized",
                        "root",
                        "metadata",
                        "list",
                        "request_status",
                        "create_folder",
                        "create_file",
                        "update",
                        "relocate",
                        "delete",
                        "read",
                        "read_text",
                        "search",
                        "collect",
                        "fence",
                        "purge"
                      ],
                      "reason": [
                        "invalid",
                        "not_found",
                        "conflict",
                        "denied",
                        "fenced",
                        "storage_failure",
                        "metadata_failure",
                        "blob_failure",
                        "encoding_failure"
                      ]
                    },
                    "required": [
                      "operation",
                      "reason"
                    ]
                  },
                  [
                    null
                  ]
                ]
              }
            },
            "required": [
              "operation",
              "code",
              "classification"
            ]
          },
          [
            null
          ]
        ]
      }
    },
    "reason_fields": [
      "reason"
    ],
    "reasons": [
      "authorization_denied",
      "authorization_failed",
      "conflict",
      "denied",
      "invalid",
      "limit",
      "not_found",
      "outcome_unknown",
      "unavailable"
    ],
    "required": [
      "operation",
      "reason"
    ],
    "version": 1
  }
} as const;
