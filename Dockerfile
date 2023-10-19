FROM rust AS builder

RUN cargo new /app/
COPY Cargo.toml Cargo.lock /app/
WORKDIR /app/
RUN cargo build --release --locked # Create a new layer with all dependencies downloaded and built

COPY ./ /app/
RUN cargo build --release --locked # Build the app itself

FROM debian:buster-slim
COPY --from=builder /app/target/release/filedl /app/
CMD ["/app/filedl"]
