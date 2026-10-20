package main

import (
	"context"
	"encoding/json"
	"errors"
	"github.com/aexhq/brain"
)

type sessionReader interface {
	ReadSession(context.Context, string, string) (brain.Session, error)
	ReadEvents(context.Context, string, string, string, int) ([]brain.Event, error)
}

func sessionProvider(authorize func(context.Context, brain.BindingContext) error, reader func() sessionReader) brain.ToolProvider {
	return brain.ToolProvider{Parameters: json.RawMessage(`{"type":"object"}`), Authorize: authorize, Bind: func(_ context.Context, b brain.BindingContext) ([]brain.ManagedTool, error) {
		inspect := brain.ManagedTool{Tool: brain.Tool{Name: "inspect", Description: "Read the current session metadata", InputSchema: json.RawMessage(`{"type":"object","additionalProperties":false}`)}, Handler: func(ctx context.Context, _ brain.ToolRequest, _ func(json.RawMessage)) brain.Result {
			session, err := reader().ReadSession(ctx, b.Tenant, b.Session)
			return inspection(session, err)
		}}
		history := brain.ManagedTool{Tool: brain.Tool{Name: "history", Description: "Read retained events from this session using an opaque cursor", InputSchema: json.RawMessage(`{"type":"object","properties":{"after":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}},"additionalProperties":false}`)}, Handler: func(ctx context.Context, request brain.ToolRequest, _ func(json.RawMessage)) brain.Result {
			input := struct {
				After string `json:"after"`
				Limit int    `json:"limit"`
			}{Limit: 20}
			if err := json.Unmarshal(request.Arguments, &input); err != nil {
				return brain.ToolError("invalid history query", false)
			}
			events, err := reader().ReadEvents(ctx, b.Tenant, b.Session, input.After, input.Limit)
			if err != nil {
				return inspection(nil, err)
			}
			cursor := input.After
			if len(events) > 0 {
				cursor = events[len(events)-1].Cursor
			}
			return inspection(struct {
				Events []brain.Event `json:"events"`
				Next   string        `json:"next_cursor"`
			}{events, cursor}, nil)
		}}
		return []brain.ManagedTool{inspect, history}, nil
	}}
}
func inspection(value any, err error) brain.Result {
	if err != nil {
		return brain.ToolError("session inspection failed", false)
	}
	body, err := json.Marshal(value)
	if len(body) > 256<<10 {
		err = errors.New("inspection output exceeds limit")
	}
	if err != nil {
		return brain.ToolError("inspection output exceeds limit; request fewer events", false)
	}
	return brain.Result{Content: []brain.Content{{Type: "text", Text: string(body)}}}
}
