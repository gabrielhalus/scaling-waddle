FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p sw-api -p sw-worker

FROM debian:bookworm-slim
RUN useradd --system --create-home --uid 10001 waddle && mkdir /data && chown waddle /data
COPY --from=build /src/target/release/sw-api /src/target/release/sw-worker /usr/local/bin/
USER waddle
VOLUME /data
ENV SW_DATABASE_PATH=/data/scaling-waddle.db SW_BIND=0.0.0.0:8080 SW_LOG_FORMAT=json
EXPOSE 8080
CMD ["sw-api"]
