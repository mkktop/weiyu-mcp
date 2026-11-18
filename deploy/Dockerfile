# syntax=docker/dockerfile:1
# Runtime image is configurable so restricted networks can use a registry mirror.
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian12:nonroot

FROM rust:1-bookworm AS builder

ARG CARGO_BUILD_JOBS=2
ARG CARGO_PROFILE_RELEASE_LTO=thin
ARG CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS} \
    CARGO_PROFILE_RELEASE_LTO=${CARGO_PROFILE_RELEASE_LTO} \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=${CARGO_PROFILE_RELEASE_CODEGEN_UNITS}

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --locked --release -p weiyu-mcp \
    && install -m 0755 target/release/weiyu-mcp /weiyu-mcp

FROM ${RUNTIME_IMAGE} AS runtime

# distroless does not ship CA certificates; copy the bundle from the builder so
# rustls can verify Bilibili HTTPS endpoints.
COPY --from=builder /etc/ssl/certs /etc/ssl/certs

LABEL org.opencontainers.image.title="weiyu-mcp"
LABEL org.opencontainers.image.description="Headless Bilibili query and interaction MCP server"

COPY --from=builder /weiyu-mcp /usr/local/bin/weiyu-mcp

EXPOSE 8787
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/weiyu-mcp"]
CMD ["serve", "--listen", "0.0.0.0:8787"]
