ARG PENUMBRA_VERSION=main
# ARG PENUMBRA_VERSION=v0.54.1
# Pull from Penumbra container, so we can grab a recent `pcli` without
# needing to compile from source.
FROM ghcr.io/penumbra-zone/penumbra:${PENUMBRA_VERSION} AS penumbra
FROM docker.io/rust:1-bullseye AS builder

ARG PENUMBRA_VERSION=main
RUN apt-get update && apt-get install -y \
        libssl-dev git-lfs clang
RUN git clone --depth 1 --branch "${PENUMBRA_VERSION}" https://github.com/penumbra-zone/penumbra /app/penumbra
COPY . /app/osiris
WORKDIR /app/osiris
RUN cargo build --release

FROM docker.io/debian:bullseye-slim
RUN apt-get update && apt-get install -y ca-certificates
RUN groupadd --gid 1000 penumbra \
        && useradd -m -d /home/penumbra -g 1000 -u 1000 penumbra
COPY --from=builder /app/osiris/target/release/osiris /usr/bin/osiris
COPY --from=penumbra /bin/pcli /usr/bin/pcli
WORKDIR /home/penumbra
USER penumbra
