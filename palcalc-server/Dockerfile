# syntax=docker/dockerfile:1
#
# Built by docker-compose.yml, which supplies palm-save as a NAMED build
# context (`palmsave`) so the small palcalc-server directory stays the main
# context — no giant parent directory is sent to the daemon.
#
#     cd palcalc-server && PALCALC_SAVE_DIR=/path/to/world docker compose up -d --build
#
# Both crates compile INSIDE this image (Debian bookworm), so the binary
# matches the bookworm-slim runtime glibc — independent of the host's glibc.

FROM rust:1-bookworm AS build
WORKDIR /src
# palm-save comes from the `palmsave` named context; palcalc-server is the main
# context. Lay them out two levels deep so the crate's `../../palm-save` path
# dependency resolves (palCalc/palcalc-server -> ../../palm-save).
COPY --from=palmsave . palm-save/
COPY . palCalc/palcalc-server/
WORKDIR /src/palCalc/palcalc-server
# --locked: build exactly the committed Cargo.lock (reproducible, auditable).
RUN cargo build --release --locked --bin palcalc-server

FROM debian:bookworm-slim AS runtime
# The binary dynamically links libstdc++ (the vendored ooz C++ decompressor);
# glibc/libgcc are already in the base. Create an unprivileged user and a
# /data dir it owns (a fresh named volume inherits this ownership).
RUN apt-get update \
 && apt-get install -y --no-install-recommends libstdc++6 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --no-create-home palcalc \
 && mkdir -p /data && chown palcalc:palcalc /data
COPY --from=build /src/palCalc/palcalc-server/target/release/palcalc-server /usr/local/bin/palcalc-server
USER palcalc
WORKDIR /data
EXPOSE 8123
# Config path is argv[1]; save + cert/key live under mounted volumes.
ENTRYPOINT ["palcalc-server", "/config/palcalc-server.toml"]
