use std::collections::BTreeMap;

use decodal::{Data, Engine, ImportLoader, LoadedImport};
use serde_json::{Map, Number, Value};

use crate::profile::ProfileError;

pub use builtin_source::*;

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
            Some(&DEFAULT_PATH.to_owned())
        );
        assert_eq!(
            catalog.entrypoints.get(BUILTIN_STANDALONE_PROFILE),
            Some(&"profiles/standalone.dcdl".to_owned())
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
    fn standalone_profile_inherits_default_and_overrides_workspace_policy() {
        let value = resolve_builtin_profile_artifact(BUILTIN_STANDALONE_PROFILE)
            .expect("evaluate built-in standalone")
            .expect("standalone exists");
        assert_eq!(value["slug"], "standalone");
        assert_eq!(value["model"]["ref"], "codex-oauth/gpt-5.6-sol");
        assert_eq!(value["scope"]["intent"], "workspace_write");
        assert_eq!(value["scope"]["symlink_policy"], "logical");
        assert_eq!(value["delegation_scope"]["intent"], "workspace_write");
        assert_eq!(value["delegation_scope"]["symlink_policy"], "logical");
        assert_eq!(value["feature"]["sub_worker"]["enabled"], true);
        assert_eq!(value["feature"]["memory"]["enabled"], false);
    }

    #[test]
    fn imports_cannot_escape_the_builtin_resource_catalog() {
        assert_eq!(
            resolve_import_path("profiles/default.dcdl", "./base.dcdl").as_deref(),
            Some("profiles/base.dcdl")
        );
        assert_eq!(
            resolve_import_path("profiles/standalone.dcdl", "./default.dcdl").as_deref(),
            Some(DEFAULT_PATH)
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
