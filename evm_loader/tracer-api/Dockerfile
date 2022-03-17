FROM solanalabs/rust:latest AS builder

ARG NEON_REVISION
ENV NEON_REVISION $NEON_REVISION

COPY . /opt
WORKDIR /opt
RUN cargo build --release

FROM ubuntu:20.04
RUN apt-get update && apt-get install -y libssl-dev

WORKDIR /usr/sbin
COPY --from=builder /opt/target/release/neon-tracer .
CMD ["./neon-tracer"]
