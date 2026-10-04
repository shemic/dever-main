package main

import (
	"context"
	"encoding/json"
	"fmt"
	"os"

	component "dever-component"
)

func main() {
	bytes, err := os.ReadFile(os.Args[1])
	if err != nil {
		panic(err)
	}
	var manifest component.Manifest
	if err = json.Unmarshal(bytes, &manifest); err != nil {
		panic(err)
	}
	worker, err := component.NewFromManifest(manifest)
	if err == nil {
		err = worker.RegisterHandlers(map[string]func(context.Context, any, any) (any, error){
			"render": func(_ context.Context, payload any, setting any) (any, error) {
				message := payload.(map[string]any)["message"].(map[string]any)
				if message["name"] == "reject" {
					return nil, component.BusinessError{Identity: "example.Rejected", Payload: map[string]any{"reason": "rejected"}}
				}
				prefix := setting.(map[string]any)["prefix"].(string)
				return map[string]any{"text": prefix + message["name"].(string) + fmt.Sprint(message["number"])}, nil
			},
		})
	}
	if err == nil {
		err = worker.Serve(os.Stdin, os.Stdout)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "typed worker:", err)
		os.Exit(1)
	}
}
