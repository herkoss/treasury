FROM lukemathwalker/cargo-chef:latest-rust-1.94-slim-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
ENV SQLX_OFFLINE=true
COPY . .
RUN cargo build --release --bin server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --user-group --no-create-home app
COPY --from=builder /app/target/release/server /usr/local/bin/server
COPY web /srv/web
USER app
ENV SERVER_ADDR=0.0.0.0:8888 WEB_DIR=/srv/web
EXPOSE 8888
CMD ["server"]
