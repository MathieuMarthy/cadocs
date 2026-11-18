# syntax=docker/dockerfile:1
# The cadocs image: one binary on a small glibc base, with one volume on the data folder
# (docs/OPERATIONS.md). It starts as root and the binary gives root up at once (ADR 0025). `--build-arg FEATURES=s3` adds the S3 file store (ADR 0020).
# `--target binary --output <dir>` gives the binary alone, which is how releases get theirs.
# Images are pinned by digest, and Dependabot moves them.

# The front end, which the binary embeds. Its build checks the editor schema against fixtures of
# cadocs-content.
FROM node:26-slim@sha256:e6bedef217677b276c591795e169840edd9096dbf13f29df8ef8d1856b8820bc AS web
WORKDIR /src
COPY web/package.json web/package-lock.json web/
RUN npm --prefix web ci
COPY web web
COPY crates/cadocs-content/tests crates/cadocs-content/tests
RUN npm --prefix web run build

# Bookworm's glibc 2.36, the oldest a release binary asks for. The toolchain is the one CI pins.
FROM rust:1.100.0-slim-bookworm@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS build
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

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2
COPY --from=build /cadocs /usr/local/bin/cadocs
# Owned by the image's user, so a new named volume is too.
COPY --from=build --chown=nonroot:nonroot /data /data
# Root only until the binary has read its settings: it then runs as the owner of /data, or as
# `nonroot` once it has taken a /data that root owns, such as a bind mount Docker made.
USER 0:0
ENV CADOCS_DATA_DIR=/data \
    CADOCS_BIND=0.0.0.0:3000 \
    CADOCS_RUN_AS=65532:65532
VOLUME /data
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/cadocs"]
CMD ["serve"]
