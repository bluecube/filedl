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
CMD ["/bin/filedl"]
