FROM rust:1.98-bookworm AS native
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/core crates/core
COPY crates/node crates/node
RUN cargo build --release -p structured-editor-node

FROM node:26-bookworm-slim AS build
WORKDIR /app/frontend
RUN apt-get update && apt-get install -y --no-install-recommends python3 make g++ && rm -rf /var/lib/apt/lists/*
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend ./
COPY --from=native /src/target/release/libstructured_editor_node.so native/structured_editor_node.node
RUN npm run check && npm run build

FROM node:26-bookworm-slim
ENV NODE_ENV=production HOST=0.0.0.0 PORT=4321 EDITOR_ROOT=/content EDITOR_DATA_DIR=/state
WORKDIR /app/frontend
COPY --from=build /app/frontend/package.json ./
COPY --from=build /app/frontend/node_modules ./node_modules
COPY --from=build /app/frontend/dist ./dist
COPY --from=build /app/frontend/native ./native
COPY --from=build /app/frontend/scripts ./scripts
COPY --from=build /app/frontend/src/server ./src/server
EXPOSE 4321
CMD ["node", "dist/server/entry.mjs"]
