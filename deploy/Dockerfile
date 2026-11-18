# syntax=docker/dockerfile:1
FROM rust:1-bookworm AS builder

ARG TARGETARCH
WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target,id=weiyu-mcp-${TARGETARCH} \
    cargo build --locked --release -p weiyu-mcp \
    && install -m 0755 target/release/weiyu-mcp /weiyu-mcp

FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

LABEL org.opencontainers.image.title="weiyu-mcp"
LABEL org.opencontainers.image.description="Headless Bilibili query and interaction MCP server"

COPY --from=builder /weiyu-mcp /usr/local/bin/weiyu-mcp

EXPOSE 8787
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/weiyu-mcp"]
CMD ["serve", "--listen", "0.0.0.0:8787"]
