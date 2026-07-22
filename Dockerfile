# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.95.0
ARG DEBIAN_VERSION=bookworm
ARG DENO_VERSION=2.7.14
ARG NGINX_VERSION=1.27-alpine

FROM rust:${RUST_VERSION}-${DEBIAN_VERSION} AS rust-source
WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
        libssl-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY resources ./resources
COPY tests ./tests

FROM rust-source AS runtime-builder
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release -p worker-runtime --bin worker-runtime-rest-server --features ws-server,fs-store \
    && mkdir -p /out \
    && cp /src/target/release/worker-runtime-rest-server /out/worker-runtime-rest-server

FROM rust-source AS server-builder
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release -p yoi-workspace-server --bin yoi-workspace-server \
    && mkdir -p /out \
    && cp /src/target/release/yoi-workspace-server /out/yoi-workspace-server

FROM denoland/deno:${DENO_VERSION} AS web-builder
WORKDIR /src/web/workspace
COPY web/workspace ./
RUN deno task build

FROM debian:${DEBIAN_VERSION}-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        git \
        libssl3 \
        openssh-client \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --gid root --home-dir /runtime-data --shell /usr/sbin/nologin yoi \
    && mkdir -p /runtime-data /workdirs \
    && chown -R yoi:root /runtime-data /workdirs

COPY --from=runtime-builder /out/worker-runtime-rest-server /usr/local/bin/worker-runtime-rest-server

USER yoi
EXPOSE 38800
VOLUME ["/runtime-data", "/workdirs"]
ENTRYPOINT ["worker-runtime-rest-server"]
CMD ["--bind", "0.0.0.0:38800", "--display-name", "Docker Runtime", "--fs-root", "/runtime-data", "--workdir-target", "/workdirs"]

FROM debian:${DEBIAN_VERSION}-slim AS server
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
        libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10002 --gid root --home-dir /server-data --shell /usr/sbin/nologin yoi \
    && mkdir -p /server-data /workspace \
    && chown -R yoi:root /server-data /workspace

COPY --from=server-builder /out/yoi-workspace-server /usr/local/bin/yoi-workspace-server

USER yoi
EXPOSE 8787
VOLUME ["/server-data", "/workspace"]
ENTRYPOINT ["yoi-workspace-server"]
CMD ["serve", "--workspace", "/workspace", "--db", "/server-data/workspace.db", "--listen", "0.0.0.0:8787"]

FROM nginx:${NGINX_VERSION} AS webui
COPY docker/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=web-builder /src/web/workspace/build /usr/share/nginx/html
EXPOSE 80
