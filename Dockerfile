FROM rust:1-alpine AS builder

RUN apk add --no-cache build-base

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release

FROM alpine:latest

RUN apk add --no-cache ca-certificates

WORKDIR /app

RUN mkdir -p /data

COPY --from=builder /app/target/release/ImpostorBoy228 /main
COPY static ./static

ENV BIND_ADDR=0.0.0.0:911 \
    DB_PATH=/data/AC.db

VOLUME ["/data"]
EXPOSE 911

ENTRYPOINT ["/main"]

#docker run -d \
#  --name impostor \
#  -p 911:911 \
#  -e DB_PATH=/data/AC.db \
#  -v /home/dodik/EFTA:/data \
#  impostorboy228
