# syntax=docker/dockerfile:1

# Build the official Subscan Essentials UI at an exact commit. Upstream ships
# a runtime-only Dockerfile, so this image adds the reproducible source/build
# stages needed by local development.
ARG NODE_VERSION=18.20.2

FROM node:${NODE_VERSION}-alpine AS builder

ARG SUBSCAN_UI_REPOSITORY=https://github.com/subscan-explorer/subscan-essentials-ui-react.git
ARG SUBSCAN_UI_REV=9f29809fe463fdc5840a9b407cca58c574621c1d
ARG NEXT_PUBLIC_API_HOST=http://localhost:4399

RUN apk add --no-cache git

WORKDIR /app
RUN git init . \
 && git remote add origin "$SUBSCAN_UI_REPOSITORY" \
 && git fetch --depth=1 origin "$SUBSCAN_UI_REV" \
 && git checkout --detach FETCH_HEAD \
 && test "$(git rev-parse HEAD)" = "$SUBSCAN_UI_REV"

RUN npm ci
ENV NEXT_PUBLIC_API_HOST=${NEXT_PUBLIC_API_HOST}
RUN npm run build

FROM node:${NODE_VERSION}-alpine AS runtime

WORKDIR /app

ENV NODE_ENV=production \
    PORT=3000 \
    HOSTNAME=0.0.0.0 \
    NEXT_PUBLIC_API_HOST=http://localhost:4399 \
    NEXT_SHARP_PATH=/app/node_modules/sharp

RUN addgroup --system --gid 1001 nodejs \
 && adduser --system --uid 1001 nextjs

COPY --from=builder --chown=nextjs:nodejs /app/next.config.js ./next.config.js
COPY --from=builder --chown=nextjs:nodejs /app/package.json ./package.json
COPY --from=builder --chown=nextjs:nodejs /app/public ./public
COPY --from=builder --chown=nextjs:nodejs /app/.next/standalone ./
COPY --from=builder --chown=nextjs:nodejs /app/.next/static ./.next/static
COPY --chown=nextjs:nodejs docker/subscan-ui-entrypoint.sh /usr/local/bin/subscan-ui-entrypoint

USER nextjs

EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/subscan-ui-entrypoint"]
