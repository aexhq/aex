FROM node:24-bookworm-slim@sha256:a5f4366e834695d5f363b920c104953a535d3a9864d1bdf74566a0ebe01daee5 AS qualified
RUN apt-get update && apt-get install -y --no-install-recommends poppler-utils procps util-linux && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/aex-media
COPY services/media/ ./
RUN node --test server.test.mjs render.test.mjs
FROM qualified AS runtime
CMD ["node", "server.mjs"]
