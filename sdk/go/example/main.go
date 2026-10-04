package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"time"

	component "dever-component"
)

type payload struct {
	Text    string
	DelayMS int64
}

func setting(raw any) (string, error) {
	object, ok := raw.(map[string]any)
	if !ok || len(object) != 1 {
		return "", errors.New("invalid setting")
	}
	value, ok := object["prefix"].(string)
	if !ok {
		return "", errors.New("invalid setting")
	}
	return value, nil
}

func decode(raw any) (payload, error) {
	object, ok := raw.(map[string]any)
	if !ok || len(object) != 2 {
		return payload{}, errors.New("invalid payload")
	}
	text, ok := object["text"].(string)
	if !ok {
		return payload{}, errors.New("invalid payload")
	}
	delay, ok := object["delay_ms"].(interface{ Int64() (int64, error) })
	if !ok {
		return payload{}, errors.New("invalid payload")
	}
	milliseconds, err := delay.Int64()
	if err != nil || milliseconds < 0 || milliseconds > 10000 {
		return payload{}, errors.New("invalid payload")
	}
	return payload{Text: text, DelayMS: milliseconds}, nil
}

func main() {
	worker, err := component.New(component.Contract{
		Port: "example.Text", Schema: "fixture-schema-v1", Adapter: "example.TextAdapter",
		Operations: []string{"text.render", "text.fail", "number.echo", "text.stubborn"}, Errors: []string{"example.rejected"},
	}, setting)
	if err == nil {
		err = component.RegisterTyped(worker, "text.render", decode, func(ctx context.Context, value payload, prefix string) (any, error) {
			select {
			case <-ctx.Done():
				return nil, ctx.Err()
			case <-time.After(time.Duration(value.DelayMS) * time.Millisecond):
				return map[string]any{"value": prefix + value.Text}, nil
			}
		})
	}
	if err == nil {
		err = component.RegisterTyped(worker, "text.fail", decode, func(context.Context, payload, string) (any, error) {
			return nil, component.BusinessError{Identity: "example.rejected", Payload: map[string]any{"reason": "rejected"}}
		})
	}
	if err == nil {
		err = component.RegisterTyped(worker, "number.echo", func(raw any) (json.Number, error) {
			object, ok := raw.(map[string]any)
			if !ok || len(object) != 1 {
				return "", errors.New("invalid number")
			}
			number, ok := object["number"].(json.Number)
			if !ok {
				return "", errors.New("invalid number")
			}
			if _, err := number.Int64(); err != nil {
				return "", errors.New("invalid number")
			}
			return number, nil
		}, func(_ context.Context, value json.Number, _ string) (any, error) {
			return map[string]any{"number": value}, nil
		})
	}
	if err == nil {
		err = component.RegisterTyped(worker, "text.stubborn", decode, func(_ context.Context, value payload, prefix string) (any, error) {
			time.Sleep(5 * time.Second)
			return map[string]any{"value": prefix + value.Text}, nil
		})
	}
	if err == nil {
		err = worker.Serve(os.Stdin, os.Stdout)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "component worker:", err)
		os.Exit(1)
	}
}
