# SPDX-License-Identifier: AGPL-3.0-or-later
# 知页自托管同步服务。构建上下文为仓库根目录：
#   docker build -f deploy/sync/Dockerfile -t zhiye-sync-server .
# syntax=docker/dockerfile:1

FROM rust:1.100.0-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
# 桌面程序是工作区成员，只需其清单即可解析工作区，不会被编译。
COPY apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/Cargo.toml
RUN mkdir -p apps/desktop/src-tauri/src && echo 'fn main() {}' > apps/desktop/src-tauri/src/main.rs
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p zhiye-sync-server \
    && cp target/release/zhiye-sync-server /usr/local/bin/zhiye-sync-server

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /data --no-create-home zhiye \
    && mkdir -p /data && chown zhiye /data
COPY --from=build /usr/local/bin/zhiye-sync-server /usr/local/bin/zhiye-sync-server
COPY LICENSE THIRD_PARTY_NOTICES.md /usr/share/doc/zhiye-sync-server/
USER zhiye
VOLUME /data
EXPOSE 8787
ENTRYPOINT ["zhiye-sync-server"]
CMD ["--data", "/data", "--listen", "0.0.0.0:8787", "--behind-tls-proxy"]
