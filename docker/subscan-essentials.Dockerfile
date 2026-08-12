# subscan-essentials backend (API server, subscribe and worker processes share
# this image; the compose file selects the process via `command`).
#
# Upstream publishes no Docker images, so we build from a pinned git commit.
# Upstream's own Dockerfile pins golang:1.24 while go.mod on master requires
# Go 1.25 — use golang:1.25 here instead of relying on toolchain auto-download.
ARG SUBSCAN_REF=bcb39fbd30ea3017635021f0eb1c87f6f9fd7bff

FROM golang:1.25-bookworm AS builder
ARG SUBSCAN_REF
WORKDIR /subscan
RUN git clone https://github.com/subscan-explorer/subscan-essentials.git . \
    && git checkout "${SUBSCAN_REF}"
# Spike patch: the quip runtime serves metadata v16 via state_getMetadata,
# which scale.go cannot decode (v15 max). Fetch v15 explicitly through the
# Metadata_metadata_at_version runtime API instead. See
# docker/subscan-essentials/metadata_at_version.go.
COPY docker/subscan-essentials/metadata_at_version.go internal/service/
RUN sed -i \
      -e 's|rpc.GetMetadataByHash(nil, hash...)|getMetadataAtVersionV15(s.dbStorage.RPCPool().Conn, hash...)|' \
      -e '\|"github.com/itering/substrate-api-rpc/rpc"|d' \
      internal/service/runtime.go
# Spike patch: the runtime has pallet-revive, which makes the EVM plugin
# auto-enable (plugins/evm InitDao checks for a Revive/EVM pallet) and spam a
# failing job per block — this stack has no Ethereum RPC sidecar, so drop the
# plugin from the registry.
RUN sed -i \
      -e '/registerNative(evm.New())/d' \
      -e '\|"github.com/itering/subscan/plugins/evm"|d' \
      plugins/registry.go
WORKDIR /subscan/cmd
RUN go build -o subscan

FROM alpine:3
WORKDIR /subscan
COPY --from=builder /subscan/configs configs
# The service panics without configs/config.yaml (configs.Init); upstream
# seeds it from the example the same way.
RUN cp configs/config.yaml.example configs/config.yaml
COPY --from=builder /subscan/cmd/subscan cmd/subscan
WORKDIR /subscan/cmd
RUN apk update && apk add --no-cache gcompat
ENTRYPOINT ["/subscan/cmd/subscan"]
EXPOSE 4399
