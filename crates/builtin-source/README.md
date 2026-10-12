# builtin-source

Catalog of public Profile sources embedded at compile time and shared across native/WASM. It exposes only explicit assets and does not discover sources from host files or the network.

## Main entry points

- [`BUILTIN_PROFILE_RESOURCES`] and [`BuiltinProfileResource`] — sources and import relationships
- [`builtin_profile_catalog_snapshot`] and [`BuiltinProfileCatalogSnapshot`] — fixed catalog contents and content identity
