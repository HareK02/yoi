Merged orchestration implementation into develop and validated.

Implementation delivered Plugin Tool surface registration from enabled Plugin packages:
- feature-gated Plugin Tool registration through the existing ToolRegistry/model-visible schema path
- Plugin origin metadata on Tool metadata
- discovery-only packages remain inactive without explicit enablement
- duplicate/colliding Tool names and invalid schemas fail closed
- recursive schema validation rejects nested invalid schema nodes before registration
- runtime execution remains an unavailable/runtime-missing stub; WASM runtime and grant enforcement remain follow-up Tickets

Validation on develop after merge:
- cargo test -p pod plugin::tests --no-default-features
- cargo test -p manifest plugin --no-default-features
- cargo check -p pod -p manifest -p llm-worker
- cargo build -p yoi
- target/debug/yoi ticket doctor
- nix build .#yoi --no-link
