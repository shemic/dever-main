package main

import (
	"bytes"
	"compress/gzip"
	"context"
	"crypto/sha256"
	"errors"
	"io"
	"math/big"
)

func Probe(_ context.Context, _ any, _ any) (any, error) {
	message := []byte("独立标准库")
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
	value, okay := new(big.Int).SetString("9007199254740993", 10)
	if !okay || value.Add(value, big.NewInt(1)).String() != "9007199254740994" || !bytes.Equal(decoded, message) {
		return nil, errors.New("packaged standard library result differs")
	}
	digest := sha256.Sum256(message)
	return map[string]any{"okay": len(digest) == 32}, nil
}
