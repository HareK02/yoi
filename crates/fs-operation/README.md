# fs-operation

Provides provider-side filesystem operations, bounded search, and pure text editing. Access checks and I/O for logical paths go through the provider's `FsAccessPolicy`. Does not own Workdir identity, Runtime transport, or Tool read history.

## Reference index

- [`FsAccessPolicy`] — provider access, I/O, and resource limits.
- [`CheckedTarget`] — checked saves retaining descriptors and identity.
- [`run_list`] — bounded live pagination.
- [`run_glob`] / [`run_grep`] — provider-side search.
- [`text`] — shared argument, UTF-8 byte limit, and edit result handling.
