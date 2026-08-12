# subscan-essentials React UI (Next.js standalone server).
#
# Upstream's Dockerfile expects a pre-built .next/standalone directory that is
# not checked into the repo, so we run the full build here from a pinned git
# commit and then follow upstream's runtime layout.
ARG SUBSCAN_UI_REF=9f29809fe463fdc5840a9b407cca58c574621c1d

FROM node:18.20.2-alpine AS builder
ARG SUBSCAN_UI_REF
# Baked into the client bundle as a fallback; next-runtime-env lets the
# runtime NEXT_PUBLIC_API_HOST (set in the compose file) override it.
ARG NEXT_PUBLIC_API_HOST=http://localhost:4399
ENV NEXT_PUBLIC_API_HOST=${NEXT_PUBLIC_API_HOST}
WORKDIR /app
RUN apk add --no-cache git \
    && git clone https://github.com/subscan-explorer/subscan-essentials-ui-react.git . \
    && git checkout "${SUBSCAN_UI_REF}"
RUN npm ci
RUN npm run build

FROM node:18.20.2-alpine AS runner
WORKDIR /app
ENV NODE_ENV=production
RUN addgroup -g 1001 -S nodejs && adduser -S nextjs -u 1001
COPY --from=builder /app/next.config.js ./
COPY --from=builder /app/public ./public
COPY --from=builder /app/package.json ./package.json
ENV NEXT_SHARP_PATH=/app/node_modules/sharp
COPY --from=builder --chown=nextjs:nodejs /app/.next/standalone ./
COPY --from=builder --chown=nextjs:nodejs /app/.next/static ./.next/static
USER nextjs
EXPOSE 3000
ENV PORT=3000
CMD ["node", "server.js"]
