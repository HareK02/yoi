//! Explicit compile-time public Decodal source catalog, shared by native and WASM hosts.
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const BUILTIN_PROFILE_CATALOG_ID: &str = "builtin-profiles-v2";
pub const BUILTIN_DEFAULT_PROFILE: &str = "builtin:default";
pub const BUILTIN_STANDALONE_PROFILE: &str = "builtin:standalone";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinProfileImport {
    pub specifier: &'static str,
    pub resolved_path: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinProfileResource {
    pub selector: Option<&'static str>,
    pub path: &'static str,
    pub source: &'static str,
    pub description: &'static str,
    pub imports: &'static [BuiltinProfileImport],
}

const BASE_PATH: &str = "profiles/base.dcdl";
pub const DEFAULT_PATH: &str = "profiles/default.dcdl";
const BASE_IMPORT: &[BuiltinProfileImport] = &[BuiltinProfileImport {
    specifier: "./base.dcdl",
    resolved_path: BASE_PATH,
}];
const DEFAULT_IMPORT: &[BuiltinProfileImport] = &[BuiltinProfileImport {
    specifier: "./default.dcdl",
    resolved_path: DEFAULT_PATH,
}];
const NO_IMPORTS: &[BuiltinProfileImport] = &[];

pub const BUILTIN_PROFILE_RESOURCES: &[BuiltinProfileResource] = &[
    BuiltinProfileResource {
        selector: None,
        path: BASE_PATH,
        source: include_str!("../../../resources/profiles/base.dcdl"),
        description: "Shared built-in Profile defaults.",
        imports: NO_IMPORTS,
    },
    BuiltinProfileResource {
        selector: Some(BUILTIN_DEFAULT_PROFILE),
        path: DEFAULT_PATH,
        source: include_str!("../../../resources/profiles/default.dcdl"),
        description: "Default Yoi coding profile.",
        imports: BASE_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some(BUILTIN_STANDALONE_PROFILE),
        path: "profiles/standalone.dcdl",
        source: include_str!("../../../resources/profiles/standalone.dcdl"),
        description: "Standalone Yoi coding profile.",
        imports: DEFAULT_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some("builtin:standalone-subjektiv"),
        path: "profiles/standalone-subjektiv.dcdl",
        source: include_str!("../../../resources/profiles/standalone-subjektiv.dcdl"),
        description: "Standalone with an explicitly connected local Subject.",
        imports: &[BuiltinProfileImport {
            specifier: "./standalone.dcdl",
            resolved_path: "profiles/standalone.dcdl",
        }],
    },
    BuiltinProfileResource {
        selector: Some("builtin:standalone-subjektiv-consolidation"),
        path: "profiles/standalone-subjektiv-consolidation.dcdl",
        source: include_str!("../../../resources/profiles/standalone-subjektiv-consolidation.dcdl"),
        description: "Delegated local Subject Job policy.",
        imports: &[BuiltinProfileImport {
            specifier: "./job.dcdl",
            resolved_path: "profiles/job.dcdl",
        }],
    },
    BuiltinProfileResource {
        selector: Some("builtin:ticket-worker"),
        path: "profiles/ticket-worker.dcdl",
        source: include_str!("../../../resources/profiles/ticket-worker.dcdl"),
        description: "General intent-led Ticket work with optional result recording and authorized conclusion decisions.",
        imports: &[BuiltinProfileImport {
            specifier: "./default.dcdl",
            resolved_path: DEFAULT_PATH,
        }],
    },
    BuiltinProfileResource {
        selector: Some("builtin:coder"),
        path: "profiles/coder.dcdl",
        source: include_str!("../../../resources/profiles/coder.dcdl"),
        description: "Code implementation with optional Git/MR review recipe; no universal Ticket conclusion gate.",
        imports: BASE_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some("builtin:companion"),
        path: "profiles/companion.dcdl",
        source: include_str!("../../../resources/profiles/companion.dcdl"),
        description: "General assistance with Workspace tools.",
        imports: BASE_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some("builtin:intake"),
        path: "profiles/intake.dcdl",
        source: include_str!("../../../resources/profiles/intake.dcdl"),
        description: "Read-only intake and planning.",
        imports: BASE_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some("builtin:reviewer"),
        path: "profiles/reviewer.dcdl",
        source: include_str!("../../../resources/profiles/reviewer.dcdl"),
        description: "Independent review of a published Merge Request source.",
        imports: BASE_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some("builtin:orchestrator"),
        path: "profiles/orchestrator.dcdl",
        source: include_str!("../../../resources/profiles/orchestrator.dcdl"),
        description: "Intent-led Ticket orchestration with separate guarded MR integration authority.",
        imports: BASE_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some("builtin:job"),
        path: "profiles/job.dcdl",
        source: include_str!("../../../resources/profiles/job.dcdl"),
        description: "Result-only caller-selected Job policy.",
        imports: NO_IMPORTS,
    },
    BuiltinProfileResource {
        selector: Some("builtin:backend-job"),
        path: "profiles/backend-job.dcdl",
        source: include_str!("../../../resources/profiles/backend-job.dcdl"),
        description: "Backend-owned bounded Job Worker.",
        imports: BASE_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some("builtin:memory-consolidation"),
        path: "profiles/memory-consolidation.dcdl",
        source: include_str!("../../../resources/profiles/memory-consolidation.dcdl"),
        description: "Internal Memory consolidation service.",
        imports: BASE_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some("builtin:subjektiv-memory-consolidation"),
        path: "profiles/subjektiv-memory-consolidation.dcdl",
        source: include_str!("../../../resources/profiles/subjektiv-memory-consolidation.dcdl"),
        description: "Delegated subject Memory consolidation service.",
        imports: BASE_IMPORT,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinProfileCatalogSnapshot {
    pub id: &'static str,
    pub sources: BTreeMap<String, String>,
    pub entrypoints: BTreeMap<String, String>,
    pub imports: BTreeMap<String, String>,
}

impl BuiltinProfileCatalogSnapshot {
    pub fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.id.as_bytes());
        for (path, source) in &self.sources {
            hasher.update((path.len() as u64).to_le_bytes());
            hasher.update(path.as_bytes());
            hasher.update((source.len() as u64).to_le_bytes());
            hasher.update(source.as_bytes());
        }
        for (selector, path) in &self.entrypoints {
            hasher.update((selector.len() as u64).to_le_bytes());
            hasher.update(selector.as_bytes());
            hasher.update((path.len() as u64).to_le_bytes());
            hasher.update(path.as_bytes());
        }
        for (request, resolved_path) in &self.imports {
            hasher.update((request.len() as u64).to_le_bytes());
            hasher.update(request.as_bytes());
            hasher.update((resolved_path.len() as u64).to_le_bytes());
            hasher.update(resolved_path.as_bytes());
        }
        format!(
            "sha256:{}",
            hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    }
}

pub fn builtin_profile_catalog_snapshot() -> BuiltinProfileCatalogSnapshot {
    let mut sources = BTreeMap::new();
    let mut entrypoints = BTreeMap::new();
    let mut imports = BTreeMap::new();

    for resource in BUILTIN_PROFILE_RESOURCES {
        sources.insert(resource.path.to_owned(), resource.source.to_owned());
        for import in resource.imports {
            imports.insert(
                format!("{}\0{}", resource.path, import.specifier),
                import.resolved_path.to_owned(),
            );
        }
        if let Some(selector) = resource.selector {
            entrypoints.insert(selector.to_owned(), resource.path.to_owned());
        }
    }

    BuiltinProfileCatalogSnapshot {
        id: BUILTIN_PROFILE_CATALOG_ID,
        sources,
        entrypoints,
        imports,
    }
}

pub fn builtin_profile_entrypoints() -> impl Iterator<Item = &'static BuiltinProfileResource> {
    BUILTIN_PROFILE_RESOURCES
        .iter()
        .filter(|resource| resource.selector.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn ticket_worker_catalog_import_graph_includes_default_and_base() {
        // This snapshot supplies the Runtime profile source archive. Follow its
        // declared edges rather than resolving against every catalog source.
        let catalog = builtin_profile_catalog_snapshot();
        let entrypoint = catalog.entrypoints["builtin:ticket-worker"].clone();
        assert_eq!(
            catalog
                .imports
                .get(&format!("{entrypoint}\0./default.dcdl")),
            Some(&DEFAULT_PATH.to_owned())
        );
        assert_eq!(
            catalog.imports.get(&format!("{DEFAULT_PATH}\0./base.dcdl")),
            Some(&BASE_PATH.to_owned())
        );

        let mut pending = vec![entrypoint.clone()];
        let mut reachable = BTreeSet::new();
        while let Some(path) = pending.pop() {
            if !reachable.insert(path.clone()) {
                continue;
            }
            assert!(catalog.sources.contains_key(&path), "missing source {path}");
            let prefix = format!("{path}\0");
            pending.extend(
                catalog
                    .imports
                    .iter()
                    .filter(|(request, _)| request.starts_with(&prefix))
                    .map(|(_, resolved)| resolved.clone()),
            );
        }
        assert_eq!(
            reachable,
            BTreeSet::from([entrypoint, DEFAULT_PATH.to_owned(), BASE_PATH.to_owned()])
        );
    }
}
