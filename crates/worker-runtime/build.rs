// Validate embedded resources with the same sealed archive reader used at runtime.
// Keeping this module shared prevents a permissive build-only import loader from
// accepting a catalog that the Runtime would reject.
#[allow(dead_code)]
#[path = "src/profile_archive.rs"]
mod profile_archive;

fn main() {
    println!("cargo:rerun-if-changed=../../resources/profiles");
    println!("cargo:rerun-if-changed=../builtin-source/src/lib.rs");
    println!("cargo:rerun-if-changed=src/profile_archive.rs");
    let catalog = manifest::builtin_profile_catalog_snapshot();
    let archive =
        profile_archive::ProfileSourceArchive::build(profile_archive::ProfileSourceArchiveInput {
            id: catalog.id.to_owned(),
            entrypoints: catalog.entrypoints,
            imports: catalog.imports,
            sources: catalog.sources,
        })
        .and_then(|archive| archive.verify())
        .expect("invalid embedded Profile source archive");
    for selector in archive.reference().source_graph.entrypoints.keys() {
        archive
            .resolve_profile(
                selector,
                std::path::Path::new("/"),
                "builtin-build-validation",
            )
            .unwrap_or_else(|error| panic!("invalid embedded Profile {selector}: {error}"));
    }
}
