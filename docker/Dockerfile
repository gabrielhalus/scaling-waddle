FROM node:24-bookworm-slim AS web
WORKDIR /src
RUN corepack enable
COPY package.json pnpm-workspace.yaml pnpm-lock.yaml ./
COPY apps/web/package.json apps/web/
RUN pnpm install --frozen-lockfile
COPY apps/web apps/web
RUN pnpm --filter web build

FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p sw-api -p sw-worker

FROM node:24-bookworm-slim AS agent
RUN npm install -g @anthropic-ai/claude-code && useradd --system --create-home --uid 10001 waddle && mkdir /data && chown waddle /data
COPY --from=build /src/target/release/sw-api /src/target/release/sw-worker /usr/local/bin/
COPY --from=web /src/apps/web/dist /srv/web
USER waddle
VOLUME /data
ENV SW_WEB_DIR=/srv/web SW_DATABASE_PATH=/data/scaling-waddle.db SW_BIND=0.0.0.0:8080 SW_LOG_FORMAT=json SW_AGENT_COMMAND=claude
EXPOSE 8080
CMD ["sw-api"]

FROM debian:bookworm-slim
RUN useradd --system --create-home --uid 10001 waddle && mkdir /data && chown waddle /data
COPY --from=build /src/target/release/sw-api /src/target/release/sw-worker /usr/local/bin/
COPY --from=web /src/apps/web/dist /srv/web
USER waddle
VOLUME /data
ENV SW_WEB_DIR=/srv/web SW_DATABASE_PATH=/data/scaling-waddle.db SW_BIND=0.0.0.0:8080 SW_LOG_FORMAT=json
EXPOSE 8080
CMD ["sw-api"]
