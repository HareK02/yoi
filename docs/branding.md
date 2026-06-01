# Branding notes

## Rename rationale

The project must move away from the name `insomnia`. Kong already operates a well-known developer product named Insomnia, with overlapping surfaces such as an API/developer tool product, an `insomnia` package/binary, documentation, plugins, CLI tooling, and `.insomnia` project data. Even without making a legal judgment here, keeping `insomnia` would create unnecessary user confusion and distribution risk.

The rename should not be a shallow package-name change. The public product name, CLI command, data/config directory names, environment variable prefix, documentation language, and repository-facing terminology should converge on the new name.

## Product identity

This project is not just an AI chat client. It is a local agent runtime for delegating long-running development work while preserving state, context, authority boundaries, and resumability.

The product promise is:

> You can leave the keyboard, and the system remains present with your work.

That includes:

- running and restoring Pods;
- preserving session history, memory, workflows, and WorkItems;
- coordinating delegated work through scoped permissions;
- keeping human review and approval points explicit;
- making long-running or interrupted work observable and resumable.

The name should therefore avoid generic AI-agent vocabulary such as `agent`, `forge`, `sentinel`, or `autopilot`. It should express presence, custody, watchfulness, and delegated work without implying reckless full autonomy.

## Proposed name: Yoi

The leading candidate is **Yoi**, from the archaic Japanese word **夜居**.

`夜居` means staying up or being present at night, including the sense of night duty or remaining at the workplace until late. This preserves the original emotional source of `Insomnia`—AI-induced sleeplessness and the desire for something to keep watch on the user's behalf—while moving the framing from a personal condition to a product role.

`Insomnia` described what happened to the user.

`Yoi` describes what the system does instead:

> It stays with the work after the user leaves.

## Why Yoi fits

### Short and tool-shaped

`yoi` is short, easy to type, and works naturally as a command name:

```text
yoi
yoi pod
yoi memory
yoi workflow
```

It also maps cleanly to project/runtime surfaces:

```text
.yoi/
~/.yoi/
YOI_*
```

### Japanese but not overly literal

The name has the same kind of compact Japanese-derived shape as names like Hono or Shiki. It does not sound like a generic enterprise AI product, and it leaves room for the project to develop its own vocabulary.

### Presence rather than automation

The important word in `夜居` is not only night, but **居**: to be there.

That matches the system's architecture. Pods, sessions, memory, workflow state, WorkItems, and runtime metadata remain present so the user can return to them. The product is less about a single model response and more about maintaining a durable place where delegated work can continue.

### Keeps the original story without keeping the problem

The old name centered sleeplessness. The new name centers the replacement for sleeplessness: a system that remains at the worksite on the user's behalf.

A useful internal phrasing is:

> Insomnia was the problem. Yoi is the night presence that makes it unnecessary.

## Handling ambiguity

`Yoi` is intentionally short and has Japanese homophones or near associations such as `良い`, `宵`, and `酔い`. This ambiguity is acceptable if the official spelling and explanation consistently anchor the name as **Yoi (夜居)**.

The ambiguity should be handled as follows:

- Use `Yoi` for the product name in English-facing text.
- Introduce it as `Yoi (夜居)` in README and high-level docs.
- Use copy centered on presence, delegated work, and returning later.
- Do not lean into alcohol-related `酔い` wordplay.
- Do not over-explain the name in every document; establish the origin once and then use `Yoi` normally.

## Candidate tagline directions

Possible English taglines:

- `Yoi is a local AI agent runtime that stays with your work after you leave.`
- `Yoi keeps a presence at your desk while your agents work.`
- `Yoi lets you delegate long-running development work without staying at the keyboard.`

Possible Japanese descriptions:

- `Yoi（夜居）は、あなたが離れたあとも仕事場に居続けるローカル AI エージェント実行環境です。`
- `Yoi は、Pod、履歴、メモリ、ワークフローを保ち、長く続く開発作業の委譲を支えます。`

## Rename surfaces to update

If `Yoi` is adopted, the rename should cover at least:

- product name: `Insomnia` -> `Yoi`;
- CLI command: `insomnia` -> `yoi`;
- workspace directory: `.insomnia/` -> `.yoi/`;
- user data directory: `~/.insomnia/` -> `~/.yoi/`;
- environment variables: `INSOMNIA_*` -> `YOI_*`;
- Nix package/app names;
- Cargo package names where public-facing;
- prompt and resource text;
- docs, reports, AGENTS instructions, tickets, and release material;
- socket/runtime path labels and diagnostics where user-visible.

Compatibility aliases should be considered separately, but the public identity should not preserve `insomnia` longer than necessary.

## Adoption checks

The required check for this project is Nixpkgs package-name availability, because the project is expected to be distributed as a Nix package.

Checked on 2026-06-01:

- Nixpkgs: `nix search nixpkgs '^yoi$' --json` returned no package.
- Nixpkgs: `nix eval nixpkgs#yoi.meta.name --raw` reported that `nixpkgs` does not provide `yoi`.
- Nixpkgs: a direct `<nixpkgs>` attribute check returned `false` for `builtins.hasAttr "yoi" pkgs`.
- Nixpkgs: nearby existing attributes include `yo` and `yoink`, but not `yoi`.

This satisfies the current required availability check for adopting `Yoi`.

A broader package-manager pass was also performed on 2026-06-01. Results:

- crates.io: `https://crates.io/api/v1/crates/yoi` returned 404; no exact crate found by `cargo search`.
- npm: exact package `yoi` exists. `npm view yoi` reports `name: yoi`, `version: 1.9.15`, description `Easy (but powerful) NodeJS Server`, created in 2013 and modified in 2022. No `bin` field was reported. This blocks using the exact npm package name without owner coordination, but does not appear to be a CLI-name conflict.
- PyPI: `https://pypi.org/pypi/yoi/json` returned 404.
- Homebrew formula: `https://formulae.brew.sh/api/formula/yoi.json` returned 404.
- Homebrew cask: `https://formulae.brew.sh/api/cask/yoi.json` returned 404.
- Arch official packages: exact name search returned no package.
- AUR: `https://aur.archlinux.org/rpc/v5/info/yoi` returned no result.
- Alpine packages: package-name search returned no exact `yoi` package.
- Debian packages: package-name search returned no exact `yoi` package.
- Fedora packages: `https://src.fedoraproject.org/rpms/yoi` returned 404.
- Gentoo packages: exact package pages checked under likely categories returned 404.
- MacPorts: package search returned no exact `yoi` port.
- Snap: Snapcraft API reported `No snap named 'yoi' found in series '16'.`
- Flathub: direct appstream lookup did not find an app named `yoi`.
- Scoop Main bucket: `bucket/yoi.json` returned 404.
- winget-pkgs: `manifests/y/yoi` returned 404.
- Chocolatey community package page returned 404.
- Docker Hub: `library/yoi` and `yoi/yoi` returned 404.
- RubyGems: `https://rubygems.org/api/v1/gems/yoi.json` returned 404.
- NuGet: `https://api.nuget.org/v3-flatcontainer/yoi/index.json` returned 404.
- Maven Central: artifact/group search did not show an exact `yoi` package identity.
- Packagist: `https://packagist.org/packages/yoi/yoi.json` returned 404.

The main package-manager concern found is the stale npm package. Since Nixpkgs is the required distribution target and npm distribution is not currently required, this does not block adopting `Yoi`, but it should be recorded as a known name collision if npm distribution is ever planned.

Optional broader checks before a public release may still include:

- GitHub repository/name availability;
- domain availability such as `yoi.dev`, `yoi.rs`, `yoi.sh`, or `getyoi.dev`;
- trademark search results in relevant jurisdictions.

The main risk of `Yoi` is not semantic ambiguity but short-name collision. The required Nixpkgs check is clear, and the broader package-manager pass is acceptable apart from the npm package-name collision.
