use builtin_source::{BUILTIN_PROFILE_RESOURCES, builtin_profile_catalog_snapshot};
use std::collections::BTreeSet;

#[test]
fn public_catalog_is_closed_and_explicit() {
    let catalog = builtin_profile_catalog_snapshot();
    assert_eq!(catalog.sources.len(), BUILTIN_PROFILE_RESOURCES.len());
    assert!(catalog.digest().starts_with("sha256:"));
    for resource in BUILTIN_PROFILE_RESOURCES {
        assert!(resource.path.starts_with("profiles/"));
        assert!(resource.path.ends_with(".dcdl"));
        assert_eq!(catalog.sources[resource.path], resource.source);
        for import in resource.imports {
            assert!(catalog.sources.contains_key(import.resolved_path));
            assert!(
                resource
                    .source
                    .contains(&format!("import {:?}", import.specifier))
            );
        }
    }
    assert!(!catalog.sources.contains_key("prompts/default.md"));
}

#[test]
fn catalog_import_metadata_has_no_cycles() {
    fn visit(path: &str, visiting: &mut BTreeSet<String>) {
        assert!(visiting.insert(path.to_string()), "catalog cycle at {path}");
        let resource = BUILTIN_PROFILE_RESOURCES
            .iter()
            .find(|resource| resource.path == path)
            .unwrap();
        for import in resource.imports {
            visit(import.resolved_path, visiting);
        }
        visiting.remove(path);
    }
    for resource in BUILTIN_PROFILE_RESOURCES {
        visit(resource.path, &mut BTreeSet::new());
    }
}

#[test]
fn async_chat_is_public_and_does_not_inherit_broader_profile_defaults() {
    let catalog = builtin_profile_catalog_snapshot();
    let path = &catalog.entrypoints["builtin:async-chat"];
    let resource = BUILTIN_PROFILE_RESOURCES
        .iter()
        .find(|resource| resource.path == path)
        .unwrap();
    assert_eq!(resource.selector, Some("builtin:async-chat"));
    assert!(!resource.description.is_empty());
    assert!(resource.imports.is_empty());
    assert!(
        !catalog
            .imports
            .keys()
            .any(|request| request.starts_with(&format!("{path}\0")))
    );
}
