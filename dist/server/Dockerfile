# driftling-server: сборка из корня репозитория:
#   docker build -f dist/server/Dockerfile -t driftling-server .
#
# SQLite вшит статически (rusqlite bundled), поэтому рантайм-образу
# хватает голого debian-slim без единого пакета.

FROM rust:1-slim-trixie AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p driftling-server

FROM debian:trixie-slim
# Непривилегированный пользователь; данные — на волюме /data.
RUN useradd --system --home /data --shell /usr/sbin/nologin driftling \
    && mkdir -p /data && chown driftling:driftling /data
COPY --from=build /src/target/release/driftling-server /usr/local/bin/driftling-server
COPY dist/server/docker-config.toml /etc/driftling-server.toml
USER driftling
VOLUME /data
EXPOSE 8787
ENTRYPOINT ["driftling-server", "--config", "/etc/driftling-server.toml"]
CMD ["serve"]
