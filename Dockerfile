# syntax=docker/dockerfile:1
# The cadocs image: one binary on a small glibc base, as a non-root user, with one volume on the
# data folder (docs/OPERATIONS.md). `--build-arg FEATURES=s3` adds the S3 file store (ADR 0020).
# `--target binary --output <dir>` gives the binary alone, which is how releases get theirs.
# Images are pinned by digest, and Dependabot moves them.

# The front end, which the binary embeds. Its build checks the editor schema against fixtures of
# cadocs-content.
FROM node:26-slim@sha256:5adad64f9a1fca2d51972caa65983634c8a774eb300d72b20f7f2a0f60368072 AS web
WORKDIR /src
COPY web/package.json web/package-lock.json web/
RUN npm --prefix web ci
COPY web web
COPY crates/cadocs-content/tests crates/cadocs-content/tests
RUN npm --prefix web run build

# Bookworm's glibc 2.36, the oldest a release binary asks for. The toolchain is the one CI pins.
FROM rust:1.99.1-slim-bookworm@sha256:f2c004e3d2d8b547e8cf7ffbc29205385f4e71d23e53e4e383065db5e9f822cd AS build
ARG FEATURES=""
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY --from=web /src/web/dist web/dist
RUN cargo build --release --locked -p cadocs-server --features "$FEATURES" \
    && cp target/release/cadocs /cadocs \
    && mkdir /data

FROM scratch AS binary
COPY --from=build /cadocs /cadocs

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cc66d222f0a7ec996b78a99fe76a969bc80b539a5016ad866c14d1949bc6d0ab
COPY --from=build /cadocs /usr/local/bin/cadocs
# Owned by the image's user, so a new named volume is too.
COPY --from=build --chown=nonroot:nonroot /data /data
ENV CADOCS_DATA_DIR=/data \
    CADOCS_BIND=0.0.0.0:3000
VOLUME /data
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/cadocs"]
CMD ["serve"]
