package main

import (
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"errors"
	"io"
	"runtime"

	"github.com/google/uuid"
)

func Probe(_ context.Context, _ any, _ any) (any, error) {
	identity, err := uuid.Parse("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
	if err != nil {
		return nil, err
	}
	if runtime.GOARCH != "arm64" || identity.Version() != 1 {
		return nil, errors.New("ARM target or locked UUID library differs")
	}
	message := []byte("ARM 独立依赖")
	var encoded bytes.Buffer
	writer := gzip.NewWriter(&encoded)
	if _, err := writer.Write(message); err != nil {
		return nil, err
	}
	if err := writer.Close(); err != nil {
		return nil, err
	}
	reader, err := gzip.NewReader(&encoded)
	if err != nil {
		return nil, err
	}
	decoded, err := io.ReadAll(reader)
	if err != nil {
		return nil, err
	}
	if err := reader.Close(); err != nil {
		return nil, err
	}
	digest := sha256.Sum256(message)
	return map[string]any{"okay": bytes.Equal(decoded, message) && len(digest) == 32}, nil
}

func Reject(_ context.Context, _ any, _ any) (any, error) {
	return nil, errors.New("expected ARM Worker failure")
}
