package service

// Spike patch (quip-validator): the quip runtime serves metadata v16 via
// state_getMetadata, but scale.go (v1.10.1, latest) only decodes up to v15.
// The runtime still downgrades on request through the Metadata runtime API,
// so fetch v15 explicitly via state_call Metadata_metadata_at_version and
// unwrap the Option<OpaqueMetadata> envelope before handing the blob to
// scale.go. Drop this file once upstream scale.go supports metadata v16.

import (
	"encoding/hex"
	"encoding/json"
	"errors"
	"math/rand"
	"strings"

	"github.com/itering/substrate-api-rpc/model"
	"github.com/itering/substrate-api-rpc/rpc"
	"github.com/itering/substrate-api-rpc/websocket"
)

// metadataAtVersion is the SCALE-encoded u32 of the metadata version we ask
// the runtime for (15).
const metadataAtVersion = "0x0f000000"

func getMetadataAtVersionV15(conn websocket.WsConn, hash ...string) (string, error) {
	params := []string{"Metadata_metadata_at_version", metadataAtVersion}
	if len(hash) > 0 && hash[0] != "" {
		params = append(params, hash[0])
	}
	query, err := json.Marshal(rpc.Param{
		Id:      rand.Intn(10000),
		Method:  "state_call",
		Params:  params,
		JsonRpc: "2.0",
	})
	if err != nil {
		return "", err
	}
	v := &model.JsonRpcResult{}
	if err = websocket.SendWsRequest(conn, v, query); err != nil {
		return "", err
	}
	raw, err := v.ToString()
	if err != nil {
		return "", err
	}
	return unwrapOpaqueMetadata(raw)
}

// unwrapOpaqueMetadata decodes Option<OpaqueMetadata> (Some flag + compact
// length prefix + bytes) and returns the inner metadata blob as 0x-hex.
func unwrapOpaqueMetadata(raw string) (string, error) {
	if !strings.HasPrefix(raw, "0x") {
		return "", errors.New("unexpected state_call result encoding")
	}
	b, err := hex.DecodeString(raw[2:])
	if err != nil {
		return "", err
	}
	if len(b) < 5 || b[0] != 1 {
		return "", errors.New("Metadata_metadata_at_version returned None")
	}
	b = b[1:]
	var offset int
	switch b[0] & 0b11 {
	case 0:
		offset = 1
	case 1:
		offset = 2
	case 2:
		offset = 4
	default:
		return "", errors.New("invalid compact length prefix")
	}
	b = b[offset:]
	if string(b[:4]) != "meta" {
		return "", errors.New("unwrapped blob is not metadata")
	}
	return "0x" + hex.EncodeToString(b), nil
}
