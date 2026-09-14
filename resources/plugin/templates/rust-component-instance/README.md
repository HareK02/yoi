# Rust Component Model instance Plugin template

This offline authoring template declares the proposed `yoi:plugin/instance@1.0.0` world. Yoi currently provides no Plugin installation or Worker execution path.

The example contains request/response Tool and Service ingress shapes that a future sandboxed Server Plugin platform may support. Their host API and grant metadata is inert in the current product and grants no authority.

Build with `cargo component build --release` (or the project-specific build command used by your package), then run `yoi plugin check .` and `yoi plugin pack .` against the explicit directory. Passing offline validation does not install or authorize the package.
