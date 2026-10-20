package main

import (
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/aexhq/brain"
)

func TestManagedProviderAuthorizesScopeAndSignsOnlyPrivateRequestAuthority(t *testing.T) {
	control := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/tools/authorize" || r.Header.Get("Authorization") != "Bearer "+strings.Repeat("e", 32) {
			t.Error("wrong tool authority")
		}
		var request authorization
		if err := json.NewDecoder(r.Body).Decode(&request); err != nil {
			t.Error(err)
		}
		if request.Tenant != "account" || request.Session != "session" || request.Alias != "app" || request.Provider != "application" {
			t.Error("scope changed")
		}
		w.WriteHeader(200)
		_, _ = io.WriteString(w, `{}`)
	}))
	defer control.Close()
	t.Setenv("AEX_ENVIRONMENT_TOKEN", strings.Repeat("e", 32))
	t.Setenv("AEX_TOOL_TOKEN", strings.Repeat("t", 32))
	t.Setenv("AEX_BRAIN_TOKEN", strings.Repeat("b", 32))
	cfg := applicationConfig{Config: brain.DefaultConfig(), OperatorURL: control.URL, Managed: map[string]managedService{"application": {URL: "http://127.0.0.1:8084"}}}
	providers, err := managedProviders(cfg, nil)
	if err != nil {
		t.Fatal(err)
	}
	selected := brain.BindingContext{Tenant: "account", Session: "session", Alias: "app", Credential: json.RawMessage(`{"token":"private-customer-key"}`), Parameters: json.RawMessage(`{"authorization":"grant","endpoint":"https://app.example/tools","timeout_ms":25000,"tools":[{"name":"read","description":"Read","input_schema":{"type":"object"}}]}`)}
	provider := providers["application"]
	if err := provider.Authorize(context.Background(), selected); err != nil {
		t.Fatal(err)
	}
	bindings, err := provider.Bind(context.Background(), selected)
	if err != nil || len(bindings) != 1 {
		t.Fatalf("bindings=%v error=%v", bindings, err)
	}
	binding := bindings[0]
	if binding.URL != "http://127.0.0.1:8084/application/read" || binding.TimeoutSeconds != 25 {
		t.Fatal("unreviewed endpoint or timeout")
	}
	parts := strings.Split(binding.Credential, ".")
	mac := hmac.New(sha256.New, []byte(strings.Repeat("t", 32)))
	mac.Write([]byte(parts[0]))
	signature, _ := base64.RawURLEncoding.DecodeString(parts[1])
	if !hmac.Equal(signature, mac.Sum(nil)) {
		t.Fatal("unsigned credential")
	}
	data, _ := base64.RawURLEncoding.DecodeString(parts[0])
	var signed capsule
	if err := json.Unmarshal(data, &signed); err != nil {
		t.Fatal(err)
	}
	if string(signed.Parameters) != `{"authorization":"grant"}` || string(signed.Credential) != string(selected.Credential) {
		t.Fatalf("wrong capsule: %s", data)
	}
	if strings.Contains(string(data), "input_schema") {
		t.Fatal("public descriptors bloated the private credential")
	}
}

type inspectedSession struct{ tenant, session string }

func (r *inspectedSession) ReadSession(_ context.Context, tenant, id string) (brain.Session, error) {
	r.tenant = tenant
	r.session = id
	return brain.Session{ID: id}, nil
}
func (r *inspectedSession) ReadEvents(_ context.Context, tenant, id, after string, limit int) ([]brain.Event, error) {
	r.tenant = tenant
	r.session = id
	return []brain.Event{{ID: "event", Cursor: "opaque/cursor"}}, nil
}
func TestSessionInspectionUsesBoundAuthorityAndOpaqueCursors(t *testing.T) {
	reader := &inspectedSession{}
	provider := sessionProvider(func(context.Context, brain.BindingContext) error { return nil }, func() sessionReader { return reader })
	bindings, err := provider.Bind(context.Background(), brain.BindingContext{Tenant: "own", Session: "bound"})
	if err != nil {
		t.Fatal(err)
	}
	response := bindings[1].Handler(context.Background(), brain.ToolRequest{ToolScope: brain.ToolScope{Tenant: "forged", Session: "other"}, Arguments: json.RawMessage(`{"after":"opaque/before","limit":2}`)}, nil)
	if response.IsError || reader.tenant != "own" || reader.session != "bound" || !strings.Contains(response.Content[0].Text, `"next_cursor":"opaque/cursor"`) {
		t.Fatalf("wrong inspection: %+v", response)
	}
}
