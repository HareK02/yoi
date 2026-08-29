use std::collections::BTreeMap;

use decodal::{Data, Engine, ImportLoader, LoadedImport};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};

use crate::profile::ProfileError;

pub const BUILTIN_PROFILE_CATALOG_ID: &str = "builtin-profiles-v2";
pub const BUILTIN_DEFAULT_PROFILE: &str = "builtin:default";

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
const BASE_IMPORT: &[BuiltinProfileImport] = &[BuiltinProfileImport {
    specifier: "./base.dcdl",
    resolved_path: BASE_PATH,
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
        path: "profiles/default.dcdl",
        source: include_str!("../../../resources/profiles/default.dcdl"),
        description: "Standalone Yoi coding profile.",
        imports: BASE_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some("builtin:coder"),
        path: "profiles/coder.dcdl",
        source: include_str!("../../../resources/profiles/coder.dcdl"),
        description: "Ticket implementation with direct Reviewer SubWorkers.",
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
        description: "Workspace orchestration and Worker control.",
        imports: BASE_IMPORT,
    },
    BuiltinProfileResource {
        selector: Some("builtin:memory-consolidation"),
        path: "profiles/memory-consolidation.dcdl",
        source: include_str!("../../../resources/profiles/memory-consolidation.dcdl"),
        description: "Internal Memory consolidation service.",
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
        format!("sha256:{:x}", hasher.finalize())
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

pub(crate) fn resolve_builtin_profile_artifact(
    selector: &str,
) -> Result<Option<Value>, ProfileError> {
    let catalog = builtin_profile_catalog_snapshot();
    let Some(entrypoint) = catalog.entrypoints.get(selector) else {
        return Ok(None);
    };
    let source = catalog
        .sources
        .get(entrypoint)
        .expect("built-in Profile entrypoint must name a source")
        .clone();
    let mut engine = Engine::new(BuiltinProfileImportLoader {
        sources: catalog.sources,
    });
    let module = engine
        .add_root_source(entrypoint, entrypoint, &source)
        .map_err(|error| ProfileError::BuiltinProfileEvaluation {
            selector: selector.to_owned(),
            message: format!("{error:?}"),
        })?;
    let value =
        engine
            .eval_module(module)
            .map_err(|error| ProfileError::BuiltinProfileEvaluation {
                selector: selector.to_owned(),
                message: format!("{error:?}"),
            })?;
    let data =
        engine
            .materialize(&value)
            .map_err(|error| ProfileError::BuiltinProfileEvaluation {
                selector: selector.to_owned(),
                message: format!("{error:?}"),
            })?;
    Ok(Some(data_to_json(&data)))
}

#[derive(Debug)]
struct BuiltinProfileImportLoader {
    sources: BTreeMap<String, String>,
}

impl ImportLoader for BuiltinProfileImportLoader {
    fn load(
        &mut self,
        current_key: Option<&str>,
        specifier: &str,
    ) -> decodal::Result<LoadedImport> {
        let current_key = current_key.ok_or_else(|| {
            decodal::Diagnostic::new(
                decodal::DiagnosticKind::Import,
                decodal::Span::default(),
                format!("built-in Profile import `{specifier}` has no source context"),
            )
        })?;
        let resolved = resolve_import_path(current_key, specifier).ok_or_else(|| {
            decodal::Diagnostic::new(
                decodal::DiagnosticKind::Import,
                decodal::Span::default(),
                format!("built-in Profile import `{specifier}` from `{current_key}` is invalid"),
            )
        })?;
        let source = self.sources.get(&resolved).ok_or_else(|| {
            decodal::Diagnostic::new(
                decodal::DiagnosticKind::Import,
                decodal::Span::default(),
                format!("built-in Profile import `{specifier}` from `{current_key}` was not found"),
            )
        })?;
        Ok(LoadedImport::source(
            resolved.clone(),
            resolved,
            source.clone(),
        ))
    }
}

fn resolve_import_path(current_key: &str, specifier: &str) -> Option<String> {
    let current_parent = current_key
        .rsplit_once('/')
        .map_or("", |(parent, _)| parent);
    let joined = if let Some(relative) = specifier.strip_prefix("./") {
        format!("{current_parent}/{relative}")
    } else {
        return None;
    };
    if joined
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return None;
    }
    Some(joined)
}

fn data_to_json(data: &Data) -> Value {
    match data {
        Data::Bool(value) => Value::Bool(*value),
        Data::Int(value) => Value::Number(Number::from(*value)),
        Data::Float(value) => Number::from_f64(*value)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Data::String(value) => Value::String(value.clone()),
        Data::Array(values) => Value::Array(values.iter().map(data_to_json).collect()),
        Data::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|field| (field.name.clone(), data_to_json(&field.value)))
                .collect::<Map<_, _>>(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_one_explicit_entrypoint_for_each_builtin_profile() {
        let catalog = builtin_profile_catalog_snapshot();
        assert_eq!(catalog.sources.len(), BUILTIN_PROFILE_RESOURCES.len());
        assert_eq!(catalog.entrypoints.len() + 1, catalog.sources.len());
        assert_eq!(
            catalog.entrypoints.get(BUILTIN_DEFAULT_PROFILE),
            Some(&"profiles/default.dcdl".to_owned())
        );
        assert!(catalog.digest().starts_with("sha256:"));
    }

    #[test]
    fn default_profile_evaluates_from_the_shared_resource_graph() {
        let value = resolve_builtin_profile_artifact(BUILTIN_DEFAULT_PROFILE)
            .expect("evaluate built-in default")
            .expect("default exists");
        assert_eq!(value["slug"], "default");
        assert_eq!(value["feature"]["task"]["enabled"], true);
        assert_eq!(value["feature"]["sub_worker"]["enabled"], true);
        assert_eq!(value["feature"]["memory"]["enabled"], false);
        assert_eq!(value["feature"]["ticket"]["enabled"], false);
        assert_eq!(value["feature"]["worker"]["enabled"], false);
        assert_eq!(value["feature"]["manage_workdir"]["enabled"], false);
    }

    #[test]
    fn imports_cannot_escape_the_builtin_resource_catalog() {
        assert_eq!(
            resolve_import_path("profiles/default.dcdl", "./base.dcdl").as_deref(),
            Some("profiles/base.dcdl")
        );
        assert_eq!(
            resolve_import_path("profiles/default.dcdl", "../outside.dcdl"),
            None
        );
        assert_eq!(
            resolve_import_path("profiles/default.dcdl", "/outside.dcdl"),
            None
        );
    }
}
