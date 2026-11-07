//! Source dependency regression for the neutral contracts and representative consumers. Parse Rust
//! syntax so grouped/renamed imports and fully qualified type/expression paths are checked alike.

use syn::visit::Visit;

#[derive(Default)]
struct ProviderDependencies(Vec<String>);

impl ProviderDependencies {
    fn check(&mut self, path: &[String]) {
        // These modules are provider/composition namespaces. `super::*` inside a consumer's own
        // unit tests is local access, but importing/aliasing the parent namespace is not.
        if path.windows(2).any(|pair| pair == ["feature", "builtin"])
            || path.first().is_some_and(|segment| segment == "builtin")
            || (path.first().is_some_and(|segment| segment == "super")
                && path.get(1).is_none_or(|segment| segment != "*"))
        {
            self.0.push(path.join("::"));
        }
    }

    fn use_tree(&mut self, prefix: &mut Vec<String>, tree: &syn::UseTree) {
        match tree {
            syn::UseTree::Path(path) => {
                prefix.push(path.ident.to_string());
                self.use_tree(prefix, &path.tree);
                prefix.pop();
            }
            syn::UseTree::Group(group) => {
                for item in &group.items {
                    self.use_tree(prefix, item);
                }
            }
            syn::UseTree::Name(name) => {
                prefix.push(name.ident.to_string());
                self.check(prefix);
                prefix.pop();
            }
            syn::UseTree::Rename(rename) => {
                prefix.push(rename.ident.to_string());
                self.check(prefix);
                prefix.pop();
            }
            syn::UseTree::Glob(_) => {
                prefix.push("*".into());
                self.check(prefix);
                prefix.pop();
            }
        }
    }
}

impl<'ast> Visit<'ast> for ProviderDependencies {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        self.use_tree(&mut Vec::new(), &item.tree);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.check(
            &path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>(),
        );
        syn::visit::visit_path(self, path);
    }
}

fn dependencies(source: &str) -> Vec<String> {
    let file = syn::parse_file(source).expect("valid Rust source");
    let mut visitor = ProviderDependencies::default();
    visitor.visit_file(&file);
    visitor.0
}

#[test]
fn contracts_and_consumers_do_not_depend_on_provider_namespaces() {
    for (name, source) in [
        ("contracts", include_str!("../src/feature/contracts.rs")),
        (
            "orchestration",
            include_str!("../src/feature/builtin/orchestration.rs"),
        ),
        (
            "worker_observation",
            include_str!("../src/feature/builtin/worker_observation.rs"),
        ),
    ] {
        assert_eq!(dependencies(source), Vec::<String>::new(), "{name}");
    }
}

#[test]
fn architecture_guard_detects_imports_reexports_aliases_and_qualified_provider_paths() {
    for source in [
        "use super::manage_worker::WorkerControlService;",
        "pub use super::ticket::TicketService;",
        "use crate::feature::{builtin::{manage_worker as provider, ticket::*}};",
        "use crate::feature::builtin as providers;",
        "use super as providers;",
        "type Contract = dyn crate::feature::builtin::manage_worker::WorkerControlService;",
        "fn consumer() { crate::feature::builtin::manage_worker::manage_worker_feature(); }",
    ] {
        assert!(!dependencies(source).is_empty(), "guard missed {source}");
    }
    assert!(dependencies("use ticket::TicketError; use crate::feature::contracts::TicketService; mod tests { use super::*; }").is_empty());
}
