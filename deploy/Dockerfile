# syntax=docker/dockerfile:1
FROM node:22-bookworm-slim@sha256:83f487e0a63425e5b4d146fb5e5be574bcbe1b7b843d3ebafdd95eaf7767a7e5 AS ui
WORKDIR /src/apps
RUN corepack enable && corepack prepare pnpm@10.30.3 --activate
COPY apps/package.json apps/pnpm-lock.yaml apps/pnpm-workspace.yaml ./
ARG PNPM_NETWORK_CONCURRENCY=4
ARG PNPM_FETCH_RETRIES=5
ARG PNPM_FETCH_TIMEOUT=120000
RUN --mount=type=cache,id=codexmanager-pnpm-store,target=/pnpm/store \
    npm_config_https_proxy="${HTTPS_PROXY}" npm_config_proxy="${HTTP_PROXY}" \
    npm_config_network_concurrency="${PNPM_NETWORK_CONCURRENCY}" \
    npm_config_fetch_retries="${PNPM_FETCH_RETRIES}" \
    npm_config_fetch_timeout="${PNPM_FETCH_TIMEOUT}" \
    pnpm install --frozen-lockfile --store-dir /pnpm/store
COPY apps ./
ARG PUBLIC_API_ORIGIN=http://127.0.0.1:48760
ARG PUBLIC_ADMIN_ORIGIN=http://127.0.0.1:48761
ARG PUBLIC_STATS_ORIGIN=http://127.0.0.1:48763
ENV NEXT_PUBLIC_API_ORIGIN=${PUBLIC_API_ORIGIN} \
    NEXT_PUBLIC_ADMIN_ORIGIN=${PUBLIC_ADMIN_ORIGIN} \
    NEXT_PUBLIC_STATS_ORIGIN=${PUBLIC_STATS_ORIGIN}
RUN pnpm run build

FROM rust:1-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922 AS builder
ARG TARGETARCH
ARG TARGETVARIANT
ARG CARGO_BUILD_JOBS=4
ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse CARGO_NET_RETRY=10 CARGO_HTTP_TIMEOUT=600
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY --from=ui /src/apps/out ./apps/out
RUN --mount=type=cache,id=codexmanager-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=codexmanager-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=codexmanager-cargo-target-${TARGETARCH}${TARGETVARIANT},target=/src/target \
    cargo build --locked --release -p codexmanager-service -p codexmanager-web -p codexmanager-start \
    && mkdir -p /out \
    && cp target/release/codexmanager-service target/release/codexmanager-web target/release/codexmanager-start /out/

FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171
ARG APP_VERSION=0.1.0
ARG SOURCE_REVISION=unknown
LABEL org.opencontainers.image.title="Team AI Gateway" \
      org.opencontainers.image.source="https://github.com/m1ngxiao/team-ai-gateway" \
      org.opencontainers.image.version="${APP_VERSION}" \
      org.opencontainers.image.revision="${SOURCE_REVISION}"
ENV TZ=UTC
WORKDIR /app
RUN apt-get -o Acquire::Retries=5 -o Acquire::http::Timeout=60 update \
    && DEBIAN_FRONTEND=noninteractive apt-get -o Acquire::Retries=5 -o Acquire::http::Timeout=60 install -y --no-install-recommends ca-certificates wget tzdata \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 gateway \
    && useradd --uid 10001 --gid 10001 --home-dir /app --no-create-home --shell /usr/sbin/nologin gateway \
    && mkdir -p /data && chown -R 10001:10001 /app /data
COPY --from=builder /out/ /usr/local/bin/
COPY LICENSE /usr/share/licenses/team-ai-gateway/LICENSE
ENV CODEXMANAGER_SERVICE_ADDR=0.0.0.0:48760 \
    CODEXMANAGER_WEB_ADDR=0.0.0.0:48761 \
    CODEXMANAGER_WEB_NO_OPEN=1 \
    CODEXMANAGER_DB_PATH=/data/codexmanager.db \
    CODEXMANAGER_RPC_TOKEN_FILE=/data/codexmanager.rpc-token \
    CODEXMANAGER_GATEWAY_TRACE_STDOUT=1 \
    CODEXMANAGER_GATEWAY_ERROR_STDOUT=1 \
    CODEXMANAGER_GATEWAY_TRACE_STDOUT_SLOW_MS=5000
EXPOSE 48760 48761
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \
  CMD wget --no-verbose --tries=1 --spider http://127.0.0.1:48760/health \
    && wget --no-verbose --tries=1 --spider http://127.0.0.1:48761/__auth_status || exit 1
STOPSIGNAL SIGINT
USER 10001:10001
CMD ["/usr/local/bin/codexmanager-start"]
