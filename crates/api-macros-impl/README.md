# api-macros-impl

Procedural macro implementation for `api-macros`. It validates and normalizes trait declarations before generating metadata and adapters. It does not own runtime HTTP handling or application authorization decisions.

## Main entry points

- [`api`] — declaration normalization and code generation
