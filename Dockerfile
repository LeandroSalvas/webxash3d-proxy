# Stage 1: Build the web client (TypeScript/Vite) into dist/
FROM node:22-alpine AS client-builder

WORKDIR /app/client
COPY client/package.json client/package-lock.json ./
RUN npm ci
COPY client/ ./
RUN npm run build

# Stage 2: Build the Rust proxy, embedding dist/ from stage 1
FROM rust:1.85-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY vendor-webrtc-dtls ./vendor-webrtc-dtls
COPY src ./src
COPY --from=client-builder /app/dist ./dist

RUN cargo build --release

# Stage 3: Runtime image
FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates wget && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/webxash3d-proxy /usr/local/bin/

EXPOSE 27018

ENTRYPOINT ["webxash3d-proxy"]
