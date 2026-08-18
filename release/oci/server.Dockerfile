# syntax=docker/dockerfile:1.7
FROM node:24-bookworm-slim AS web-build
WORKDIR /src/platpulse-web
COPY platpulse-web/package.json platpulse-web/package-lock.json ./
RUN npm ci
COPY platpulse-web/ ./
RUN npm run build

FROM rust:1.88-bookworm AS server-build
RUN apt-get update && apt-get install -y --no-install-recommends git ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN cargo build --locked --release -p platpulse-server

FROM debian:bookworm-slim
ARG VERSION=dev
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
LABEL org.opencontainers.image.title="PlatPulse Server" \
      org.opencontainers.image.version="$VERSION" \
      org.opencontainers.image.source="https://github.com/mowind/PlatPulse" \
      org.opencontainers.image.licenses="MIT"
RUN groupadd --gid 10001 platpulse-server \
 && useradd --uid 10001 --gid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin platpulse-server \
 && install -d -o 10001 -g 10001 -m 0700 /var/lib/platpulse /var/backups/platpulse \
 && install -d -o 10001 -g 10001 -m 0700 /etc/platpulse/secrets /var/lib/platpulse/geo \
 && install -d -o root -g root -m 0755 /usr/share/platpulse/web
COPY --from=server-build /src/target/release/platpulse-server /usr/bin/platpulse-server
COPY --from=web-build /src/platpulse-web/dist/ /usr/share/platpulse/web/
COPY crates/platpulse-server/server.example.toml /etc/platpulse/server.example.toml
RUN chmod 0755 /usr/bin/platpulse-server && chmod -R a-w /usr/share/platpulse/web /etc/platpulse/server.example.toml
USER 10001:10001
VOLUME ["/var/lib/platpulse", "/var/backups/platpulse", "/etc/platpulse/secrets", "/var/lib/platpulse/geo", "/usr/share/platpulse/web"]
EXPOSE 8080
ENTRYPOINT ["/usr/bin/platpulse-server"]
CMD ["serve", "--config", "/etc/platpulse/server.toml"]
