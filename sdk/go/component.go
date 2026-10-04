// Package component implements the Dever external Worker process protocol.
// The compiler owns schema and typed Port codecs; callers bind its identity here.
package component

import (
	"bytes"
	"context"
	"encoding/binary"
	"encoding/json"
	"errors"
	"io"
	"math"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"
)

const (
	Version     = "dever-component-1"
	maxBytes    = 16 * 1024 * 1024
	maxDepth    = 64
	maxElements = 65536
	cancelGrace = time.Second
)

var (
	decimalPattern  = regexp.MustCompile(`^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$`)
	uuidPattern     = regexp.MustCompile(`(?i)^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$`)
	timePattern     = regexp.MustCompile(`^[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]{3})?$`)
	datetimePattern = regexp.MustCompile(`^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]{1,3})?(?:Z|[+-][0-9]{2}:[0-9]{2})$`)
)

type Contract struct {
	Port, Schema, Adapter            string
	Operations, Capabilities, Errors []string
}

type TypeSchema struct {
	Type   string                `json:"type"`
	Name   string                `json:"name,omitempty"`
	Value  *TypeSchema           `json:"value,omitempty"`
	Fields map[string]TypeSchema `json:"fields,omitempty"`
}

type Manifest struct {
	Port         string                `json:"port"`
	Schema       string                `json:"schema"`
	Adapter      string                `json:"adapter"`
	Operations   []string              `json:"operations"`
	Capabilities []string              `json:"capabilities"`
	Setting      *TypeSchema           `json:"setting"`
	Inputs       map[string]TypeSchema `json:"inputs"`
	Outputs      map[string]TypeSchema `json:"outputs"`
	Errors       map[string]TypeSchema `json:"errors"`
}

type BusinessError struct {
	Identity string
	Payload  any
}

func (e BusinessError) Error() string { return e.Identity }

type handler[S any] func(context.Context, any, S) (any, error)

type Worker[S any] struct {
	contract      Contract
	decodeSetting func(any) (S, error)
	handlers      map[string]handler[S]
	inputTypes    map[string]TypeSchema
	outputTypes   map[string]TypeSchema
	errorTypes    map[string]TypeSchema
}

func NewFromManifest(manifest Manifest) (*Worker[any], error) {
	if len(manifest.Inputs) != len(manifest.Operations) || len(manifest.Outputs) != len(manifest.Operations) {
		return nil, errors.New("incomplete typed Worker contract")
	}
	for _, operation := range manifest.Operations {
		if _, ok := manifest.Inputs[operation]; !ok {
			return nil, errors.New("missing operation input contract")
		}
		if _, ok := manifest.Outputs[operation]; !ok {
			return nil, errors.New("missing operation output contract")
		}
	}
	identities := make([]string, 0, len(manifest.Errors))
	for identity := range manifest.Errors {
		identities = append(identities, identity)
	}
	sort.Strings(identities)
	worker, err := New(Contract{
		Port: manifest.Port, Schema: manifest.Schema, Adapter: manifest.Adapter,
		Operations: manifest.Operations, Capabilities: manifest.Capabilities, Errors: identities,
	}, func(value any) (any, error) {
		if manifest.Setting == nil {
			if value != nil {
				return nil, errors.New("unexpected setting")
			}
			return nil, nil
		}
		return decodeTyped(value, *manifest.Setting)
	})
	if err != nil {
		return nil, err
	}
	worker.inputTypes, worker.outputTypes, worker.errorTypes = manifest.Inputs, manifest.Outputs, manifest.Errors
	return worker, nil
}

// ServeManifest binds the checked contract to compile-time selected handlers.
func ServeManifest(manifestBytes []byte, handlers map[string]func(context.Context, any, any) (any, error), input io.Reader, output io.Writer) error {
	var manifest Manifest
	if err := json.Unmarshal(manifestBytes, &manifest); err != nil {
		return err
	}
	worker, err := NewFromManifest(manifest)
	if err != nil {
		return err
	}
	if err := worker.RegisterHandlers(handlers); err != nil {
		return err
	}
	return worker.Serve(input, output)
}

func (w *Worker[S]) RegisterHandler(operation string, run func(context.Context, any, S) (any, error)) error {
	schema, ok := w.inputTypes[operation]
	if !ok || run == nil {
		return errors.New("operation is not in a typed Worker contract")
	}
	if w.handlers[operation] != nil {
		return errors.New("duplicate component operation")
	}
	w.handlers[operation] = func(ctx context.Context, raw any, setting S) (any, error) {
		payload, err := decodeTyped(raw, schema)
		if err != nil {
			return nil, errors.New("invalid component payload")
		}
		result, err := run(ctx, payload, setting)
		if err != nil {
			var business BusinessError
			if errors.As(err, &business) {
				typeSchema, declared := w.errorTypes[business.Identity]
				if !declared {
					return nil, errors.New("undeclared business error identity")
				}
				business.Payload, err = decodeTyped(business.Payload, typeSchema)
				if err != nil {
					return nil, errors.New("invalid business error payload")
				}
				return nil, business
			}
			return nil, err
		}
		return decodeTyped(result, w.outputTypes[operation])
	}
	return nil
}

func (w *Worker[S]) RegisterHandlers(handlers map[string]func(context.Context, any, S) (any, error)) error {
	if len(handlers) != len(w.contract.Operations) {
		return errors.New("component operations are not fully registered")
	}
	for _, operation := range w.contract.Operations {
		if err := w.RegisterHandler(operation, handlers[operation]); err != nil {
			return err
		}
	}
	return nil
}

func decodeTyped(value any, schema TypeSchema) (any, error) {
	switch schema.Type {
	case "nullable":
		if value == nil {
			return nil, nil
		}
		if schema.Value == nil {
			return nil, errors.New("invalid nullable contract")
		}
		return decodeTyped(value, *schema.Value)
	case "list":
		values, ok := value.([]any)
		if !ok || schema.Value == nil {
			return nil, errors.New("expected list")
		}
		result := make([]any, len(values))
		for index, item := range values {
			decoded, err := decodeTyped(item, *schema.Value)
			if err != nil {
				return nil, err
			}
			result[index] = decoded
		}
		return result, nil
	case "record":
		object, ok := value.(map[string]any)
		if !ok {
			return nil, errors.New("expected record")
		}
		for name := range object {
			if _, ok := schema.Fields[name]; !ok {
				return nil, errors.New("unknown record field")
			}
		}
		result := make(map[string]any, len(schema.Fields))
		for name, field := range schema.Fields {
			entry, exists := object[name]
			if !exists && field.Type != "nullable" {
				return nil, errors.New("missing record field")
			}
			decoded, err := decodeTyped(entry, field)
			if err != nil {
				return nil, err
			}
			result[name] = decoded
		}
		return result, nil
	case "int64", "duration", "model_id":
		switch number := value.(type) {
		case json.Number:
			if _, err := number.Int64(); err == nil {
				return number, nil
			}
		case int64:
			return number, nil
		case int:
			return int64(number), nil
		}
	case "float64":
		switch number := value.(type) {
		case json.Number:
			parsed, err := number.Float64()
			if err == nil && !math.IsInf(parsed, 0) && !math.IsNaN(parsed) {
				return number, nil
			}
		case float64:
			if !math.IsInf(number, 0) && !math.IsNaN(number) {
				return number, nil
			}
		}
	case "bool":
		if boolean, ok := value.(bool); ok {
			return boolean, nil
		}
	case "json":
		return value, nil
	case "text", "id", "secret", "decimal", "uuid", "datetime", "date", "time":
		if text, ok := value.(string); ok {
			switch schema.Type {
			case "decimal":
				if !decimalPattern.MatchString(text) {
					return nil, errors.New("invalid decimal")
				}
			case "uuid":
				if !uuidPattern.MatchString(text) {
					return nil, errors.New("invalid UUID")
				}
			case "date":
				if _, err := time.Parse("2006-01-02", text); err != nil {
					return nil, err
				}
			case "time":
				if !timePattern.MatchString(text) {
					return nil, errors.New("invalid Time")
				}
				layout := "15:04:05"
				if len(text) == 12 {
					layout = "15:04:05.000"
				}
				if _, err := time.Parse(layout, text); err != nil {
					return nil, err
				}
			case "datetime":
				if !datetimePattern.MatchString(text) {
					return nil, errors.New("invalid DateTime")
				}
				parsed, err := time.Parse(time.RFC3339Nano, text)
				if err != nil || parsed.Nanosecond()%1_000_000 != 0 {
					return nil, errors.New("invalid DateTime")
				}
			}
			return text, nil
		}
	}
	return nil, errors.New("invalid typed Worker value")
}

func New[S any](contract Contract, decodeSetting func(any) (S, error)) (*Worker[S], error) {
	if contract.Port == "" || contract.Schema == "" || contract.Adapter == "" || len(contract.Operations) == 0 || decodeSetting == nil || !unique(contract.Operations) || !unique(contract.Capabilities) || !unique(contract.Errors) {
		return nil, errors.New("incomplete component contract")
	}
	if contract.Capabilities == nil {
		contract.Capabilities = []string{}
	}
	if contract.Errors == nil {
		contract.Errors = []string{}
	}
	contract.Operations = append([]string(nil), contract.Operations...)
	contract.Capabilities = append([]string{}, contract.Capabilities...)
	contract.Errors = append([]string{}, contract.Errors...)
	return &Worker[S]{contract: contract, decodeSetting: decodeSetting, handlers: make(map[string]handler[S])}, nil
}

func unique(values []string) bool {
	seen := make(map[string]bool, len(values))
	for _, value := range values {
		if value == "" || seen[value] {
			return false
		}
		seen[value] = true
	}
	return true
}

// RegisterTyped binds a compiler-declared operation to its application codec.
func RegisterTyped[S, P any](worker *Worker[S], operation string, decode func(any) (P, error), run func(context.Context, P, S) (any, error)) error {
	if !contains(worker.contract.Operations, operation) || worker.handlers[operation] != nil || decode == nil || run == nil {
		return errors.New("unknown or duplicate component operation")
	}
	worker.handlers[operation] = func(ctx context.Context, raw any, setting S) (any, error) {
		payload, err := decode(raw)
		if err != nil {
			return nil, errors.New("invalid component payload")
		}
		return run(ctx, payload, setting)
	}
	return nil
}

func contains(values []string, target string) bool {
	for _, value := range values {
		if value == target {
			return true
		}
	}
	return false
}

type event struct {
	message map[string]any
	err     error
}

type result struct {
	id      int64
	payload any
	err     error
}

func (w *Worker[S]) Serve(input io.Reader, output io.Writer) error {
	if len(w.handlers) != len(w.contract.Operations) {
		return errors.New("component operations are not fully registered")
	}
	messages := make(chan event, 1)
	done := make(chan struct{})
	defer close(done)
	go func() {
		for {
			message, err := readFrame(input)
			select {
			case messages <- event{message, err}:
			case <-done:
				return
			}
			if err != nil {
				return
			}
		}
	}()
	first := <-messages
	if first.err != nil {
		return first.err
	}
	setting, err := w.handshake(first.message, output)
	if err != nil {
		return err
	}
	results := make(chan result, 1)
	var cancel context.CancelFunc
	var active int64
	var activeDone chan struct{}
	defer func() {
		if active != 0 {
			_ = stopHandler(cancel, activeDone)
		}
	}()
	nextID := int64(1)
	lastCancelled := false
	for {
		select {
		case received := <-messages:
			if received.err != nil {
				return received.err
			}
			message := received.message
			kind, ok := message["kind"].(string)
			if !ok {
				return errors.New("invalid component kind")
			}
			switch kind {
			case "health":
				if !exact(message, "kind", "id") || requestID(message["id"]) != 0 {
					return errors.New("invalid component health")
				}
				if err := writeFrame(output, message); err != nil {
					return err
				}
			case "call":
				if !exact(message, "kind", "id", "operation", "payload") {
					return errors.New("invalid component call")
				}
				id := requestID(message["id"])
				operation, ok := message["operation"].(string)
				run := w.handlers[operation]
				if id != nextID || active != 0 || !ok || run == nil {
					return errors.New("invalid component call")
				}
				nextID++
				ctx, stop := context.WithCancel(context.Background())
				cancel, active = stop, id
				activeDone = make(chan struct{})
				payload := message["payload"]
				go func(finished chan struct{}) {
					defer close(finished)
					value, err := run(ctx, payload, setting)
					select {
					case results <- result{id, value, err}:
					case <-ctx.Done():
					case <-done:
					}
				}(activeDone)
			case "cancel":
				if !exact(message, "kind", "id") {
					return errors.New("invalid component cancel")
				}
				id := requestID(message["id"])
				if id <= 0 {
					return errors.New("invalid component cancel")
				}
				if id == active {
					if err := stopHandler(cancel, activeDone); err != nil {
						active = 0
						activeDone = nil
						return err
					}
					active = 0
					activeDone = nil
					lastCancelled = true
					if err := writeFrame(output, map[string]any{"kind": "error", "id": id, "error": "dever.cancelled", "payload": nil}); err != nil {
						return err
					}
				} else if id != nextID-1 || lastCancelled {
					return errors.New("unknown component cancel id")
				}
			case "shutdown":
				if !exact(message, "kind") {
					return errors.New("invalid component shutdown")
				}
				if active != 0 {
					if err := stopHandler(cancel, activeDone); err != nil {
						active = 0
						activeDone = nil
						return err
					}
					active = 0
					activeDone = nil
				}
				return writeFrame(output, map[string]any{"kind": "shutdown"})
			default:
				return errors.New("unknown component message")
			}
		case completed := <-results:
			if completed.id != active {
				continue
			}
			cancel()
			active = 0
			activeDone = nil
			lastCancelled = false
			if completed.err == nil {
				if err := writeFrame(output, map[string]any{"kind": "result", "id": completed.id, "payload": completed.payload}); err != nil {
					return err
				}
				continue
			}
			var business BusinessError
			if !errors.As(completed.err, &business) || !contains(w.contract.Errors, business.Identity) {
				return errors.New("component handler failed")
			}
			if err := writeFrame(output, map[string]any{"kind": "error", "id": completed.id, "error": business.Identity, "payload": business.Payload}); err != nil {
				return err
			}
		}
	}
}

func stopHandler(cancel context.CancelFunc, finished <-chan struct{}) error {
	cancel()
	timer := time.NewTimer(cancelGrace)
	defer timer.Stop()
	select {
	case <-finished:
		return nil
	case <-timer.C:
		return errors.New("component handler did not stop after cancellation")
	}
}

func (w *Worker[S]) handshake(hello map[string]any, output io.Writer) (S, error) {
	var zero S
	if !exact(hello, "kind", "version", "port", "schema", "adapter", "capabilities", "operations", "setting") || hello["kind"] != "hello" || hello["version"] != Version || hello["port"] != w.contract.Port || hello["schema"] != w.contract.Schema || hello["adapter"] != w.contract.Adapter || !sameNames(hello["capabilities"], w.contract.Capabilities) || !sameNames(hello["operations"], w.contract.Operations) {
		return zero, errors.New("component handshake does not match registered contract")
	}
	setting, err := w.decodeSetting(hello["setting"])
	if err != nil {
		return zero, errors.New("invalid component setting")
	}
	if err := writeFrame(output, map[string]any{"kind": "ready", "version": Version, "port": w.contract.Port, "schema": w.contract.Schema, "adapter": w.contract.Adapter, "capabilities": w.contract.Capabilities, "operations": w.contract.Operations}); err != nil {
		return zero, err
	}
	return setting, nil
}

func sameNames(raw any, expected []string) bool {
	values, ok := raw.([]any)
	if !ok || len(values) != len(expected) {
		return false
	}
	for i, value := range values {
		if value != expected[i] {
			return false
		}
	}
	return true
}

func exact(message map[string]any, names ...string) bool {
	if len(message) != len(names) {
		return false
	}
	for _, name := range names {
		if _, ok := message[name]; !ok {
			return false
		}
	}
	return true
}

func requestID(value any) int64 {
	number, ok := value.(json.Number)
	if !ok {
		return -1
	}
	id, err := number.Int64()
	if err != nil || id < 0 {
		return -1
	}
	return id
}

func readFrame(input io.Reader) (map[string]any, error) {
	var header [4]byte
	if _, err := io.ReadFull(input, header[:]); err != nil {
		return nil, errors.New("incomplete component frame")
	}
	size := binary.BigEndian.Uint32(header[:])
	if size == 0 || size > maxBytes {
		return nil, errors.New("component frame exceeds byte limit")
	}
	body := make([]byte, size)
	if _, err := io.ReadFull(input, body); err != nil {
		return nil, errors.New("incomplete component frame")
	}
	return parseJSON(body)
}

func writeFrame(output io.Writer, message map[string]any) error {
	body, err := json.Marshal(message)
	if err != nil {
		return errors.New("invalid handler response")
	}
	if len(body) == 0 || len(body) > maxBytes {
		return errors.New("component frame exceeds byte limit")
	}
	if _, err := parseJSON(body); err != nil {
		return err
	}
	var header [4]byte
	binary.BigEndian.PutUint32(header[:], uint32(len(body)))
	if err := writeAll(output, header[:]); err != nil {
		return err
	}
	return writeAll(output, body)
}

func writeAll(output io.Writer, data []byte) error {
	for len(data) > 0 {
		count, err := output.Write(data)
		if err != nil {
			return err
		}
		if count == 0 {
			return io.ErrShortWrite
		}
		data = data[count:]
	}
	return nil
}

func parseJSON(body []byte) (map[string]any, error) {
	if !utf8.Valid(body) {
		return nil, errors.New("component frame is not UTF-8")
	}
	decoder := json.NewDecoder(bytes.NewReader(body))
	decoder.UseNumber()
	remaining := maxElements
	value, err := parseValue(decoder, 0, &remaining)
	if err != nil {
		return nil, err
	}
	if _, err := decoder.Token(); err != io.EOF {
		return nil, errors.New("invalid component JSON")
	}
	object, ok := value.(map[string]any)
	if !ok {
		return nil, errors.New("component message must be an object")
	}
	return object, nil
}

func parseValue(decoder *json.Decoder, depth int, remaining *int) (any, error) {
	if depth > maxDepth || *remaining <= 0 {
		return nil, errors.New("wire JSON exceeds limit")
	}
	*remaining--
	token, err := decoder.Token()
	if err != nil {
		return nil, errors.New("invalid component JSON")
	}
	delim, ok := token.(json.Delim)
	if !ok {
		if number, isNumber := token.(json.Number); isNumber && strings.ContainsAny(string(number), ".eE") {
			value, err := strconv.ParseFloat(string(number), 64)
			if err != nil || math.IsInf(value, 0) || math.IsNaN(value) {
				return nil, errors.New("non-finite wire Float")
			}
		}
		return token, nil
	}
	if depth == maxDepth {
		return nil, errors.New("wire JSON exceeds depth limit")
	}
	switch delim {
	case '{':
		object := make(map[string]any)
		for decoder.More() {
			key, err := decoder.Token()
			if err != nil {
				return nil, errors.New("invalid component JSON")
			}
			name, ok := key.(string)
			if !ok {
				return nil, errors.New("invalid component JSON")
			}
			if _, exists := object[name]; exists {
				return nil, errors.New("duplicate JSON field")
			}
			value, err := parseValue(decoder, depth+1, remaining)
			if err != nil {
				return nil, err
			}
			object[name] = value
		}
		if end, err := decoder.Token(); err != nil || end != json.Delim('}') {
			return nil, errors.New("invalid component JSON")
		}
		return object, nil
	case '[':
		array := make([]any, 0)
		for decoder.More() {
			value, err := parseValue(decoder, depth+1, remaining)
			if err != nil {
				return nil, err
			}
			array = append(array, value)
		}
		if end, err := decoder.Token(); err != nil || end != json.Delim(']') {
			return nil, errors.New("invalid component JSON")
		}
		return array, nil
	default:
		return nil, errors.New("invalid component JSON")
	}
}
