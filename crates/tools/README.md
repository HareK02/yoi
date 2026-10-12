# tools

`tools` implements file and command operations through Host-owned `WorkdirSession`s, plus Web fetching and image output. Owns input schemas, output formatting, and change detection after reads, but not Workdir creation or connection lifetimes, Manifest permission definitions, or the Engine execution loop.

## Main entry points

- [`core_builtin_tools`](src/lib.rs) / [`routed_builtin_tools`](src/lib.rs): registration by provider capability and connection resolution.
- [`checkout`](src/checkout.rs): shared handling for route-bound native operations and regular Tools.
- [`tracker`](src/tracker.rs): read hashes and change tracking per connection generation.
- [`view_image`](src/view_image.rs): bounded byte retrieval and image detection.
