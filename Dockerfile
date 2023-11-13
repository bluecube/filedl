FROM rust:alpine AS builder

RUN apk add musl-dev

RUN cargo new /app/
WORKDIR /app/
COPY Cargo.toml Cargo.lock /app/
RUN cargo build --release --locked # Create a new layer with all dependencies downloaded and built

COPY . /app/
RUN \
    touch src/main.rs && \
    cargo build --release --locked # Build the app itself

FROM scratch
COPY --from=builder /app/target/release/filedl /bin/filedl
ENTRYPOINT ["/bin/filedl"]

ENV FILEDL_DATA_PATH=/var/data
ENV FILEDL_LINKED_OBJECTS_ROOT=/var/linked
ENV FILEDL_BIND_PORT=8080
ENV FILEDL_BIND_ADDRESS=0.0.0.0
ENV RUST_LOG=debug
VOLUME ["$FILEDL_DATA_PATH", "$FILEDL_LINKED_OBJECTS_ROOT"]
EXPOSE $FILEDL_BIND_PORT
