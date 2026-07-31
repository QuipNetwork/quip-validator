# syntax=docker/dockerfile:1

# Subscan Essentials has no maintained prebuilt image. Build the official
# source at an exact commit so local development uses reproducible indexing
# code.
ARG GO_VERSION=1.25.0
ARG ALPINE_VERSION=3.21

FROM golang:${GO_VERSION}-bookworm AS builder

ARG SUBSCAN_REPOSITORY=https://github.com/subscan-explorer/subscan-essentials.git
ARG SUBSCAN_REV=bcb39fbd30ea3017635021f0eb1c87f6f9fd7bff

# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /subscan
RUN git init . \
 && git remote add origin "$SUBSCAN_REPOSITORY" \
 && git fetch --depth=1 origin "$SUBSCAN_REV" \
 && git checkout --detach FETCH_HEAD \
 && test "$(git rev-parse HEAD)" = "$SUBSCAN_REV"

RUN go mod download
RUN cd cmd \
 && go build -trimpath -ldflags="-s -w" -o /usr/local/bin/subscan .

FROM alpine:${ALPINE_VERSION} AS runtime

RUN apk add --no-cache ca-certificates gcompat

WORKDIR /subscan
COPY --from=builder /subscan/configs /subscan/configs
RUN cp /subscan/configs/config.yaml.example /subscan/configs/config.yaml
COPY --from=builder /usr/local/bin/subscan /usr/local/bin/subscan

WORKDIR /subscan/cmd
EXPOSE 4399
ENTRYPOINT ["/usr/local/bin/subscan"]
