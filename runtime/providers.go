package main

import (
	"bytes"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"strings"
	"time"

	"github.com/aexhq/brain"
	"github.com/aexhq/brain/integrations"
)

type managedService struct {
	URL      string       `json:"url"`
	Tools    []brain.Tool `json:"tools"`
	TokenEnv string       `json:"token_env,omitempty"`
}
type applicationConfig struct {
	brain.Config
	OperatorURL string                    `json:"operator_url"`
	Managed     map[string]managedService `json:"managed_providers"`
}
type authorization struct {
	Tenant     string          `json:"tenant"`
	Session    string          `json:"session"`
	Provider   string          `json:"provider"`
	Alias      string          `json:"alias"`
	Parameters json.RawMessage `json:"parameters"`
}
type capsule struct {
	authorization
	Credential json.RawMessage `json:"credential,omitempty"`
}

func managedProviders(cfg applicationConfig, reader func() sessionReader) (map[string]brain.ToolProvider, error) {
	providers := map[string]brain.ToolProvider{}
	if len(cfg.Managed) == 0 {
		return providers, nil
	}
	authorityToken, toolToken := os.Getenv("AEX_ENVIRONMENT_TOKEN"), os.Getenv("AEX_TOOL_TOKEN")
	needsToolToken := false
	for name := range cfg.Managed {
		needsToolToken = needsToolToken || name == "application" || name == "sandbox" || name == "browser"
	}
	if len(authorityToken) < 32 || needsToolToken && (len(toolToken) < 32 || authorityToken == toolToken || toolToken == os.Getenv("AEX_BRAIN_TOKEN")) {
		return nil, errors.New("managed tools require distinct service credentials")
	}
	control, err := url.Parse(cfg.OperatorURL)
	if err != nil || control.Scheme != "http" || (control.Hostname() != "127.0.0.1" && control.Hostname() != "::1") || control.User != nil || control.RawQuery != "" || control.Fragment != "" || control.Path != "" && control.Path != "/" {
		return nil, errors.New("operator_url must be a loopback HTTP origin")
	}
	client := &http.Client{Timeout: 15 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error { return errors.New("redirect denied") }}
	for name, service := range cfg.Managed {
		if !map[string]bool{"application": true, "sandbox": true, "browser": true, "session": true, "mcp": true}[name] {
			return nil, fmt.Errorf("unknown managed provider %s", name)
		}
		endpoint, err := url.Parse(service.URL)
		if name != "session" && (err != nil || endpoint.Host == "" || endpoint.User != nil || endpoint.RawQuery != "" || endpoint.Fragment != "" || endpoint.Scheme != "http" && endpoint.Scheme != "https") {
			return nil, errors.New("invalid installed tool service URL")
		}
		authorize := func(ctx context.Context, b brain.BindingContext) error {
			body, _ := json.Marshal(authorization{Tenant: b.Tenant, Session: b.Session, Provider: name, Alias: b.Alias, Parameters: b.Parameters})
			request, err := http.NewRequestWithContext(ctx, "POST", strings.TrimRight(cfg.OperatorURL, "/")+"/tools/authorize", bytes.NewReader(body))
			if err != nil {
				return err
			}
			request.Header.Set("Authorization", "Bearer "+authorityToken)
			request.Header.Set("Content-Type", "application/json")
			response, err := client.Do(request)
			if err != nil {
				return &brain.ProviderUnavailable{Message: "tool authorization is unavailable"}
			}
			defer response.Body.Close()
			_, _ = io.Copy(io.Discard, io.LimitReader(response.Body, 65536))
			if response.StatusCode != 200 {
				return errors.New("tool grant is unavailable")
			}
			return nil
		}
		if name == "session" {
			providers[name] = sessionProvider(authorize, reader)
			continue
		}
		if name == "mcp" {
			if service.TokenEnv != "" && os.Getenv(service.TokenEnv) == "" {
				return nil, errors.New("configured MCP credential is missing")
			}
			names := make([]string, 0, len(service.Tools))
			for _, tool := range service.Tools {
				names = append(names, tool.Name)
			}
			providers[name] = integrations.MCP(integrations.ProviderOptions{Parameters: json.RawMessage(`{"type":"object"}`), Authorize: authorize, Resolve: func(_ context.Context, b brain.BindingContext) (integrations.Endpoint, error) {
				token := os.Getenv(service.TokenEnv)
				if len(b.Credential) > 0 {
					var credential struct {
						Token string `json:"token"`
					}
					decoder := json.NewDecoder(bytes.NewReader(b.Credential))
					decoder.DisallowUnknownFields()
					if err := decoder.Decode(&credential); err != nil || credential.Token == "" {
						return integrations.Endpoint{}, errors.New("invalid MCP credential")
					}
					token = credential.Token
				}
				return integrations.Endpoint{URL: service.URL, Token: token}, nil
			}}, names)
			continue
		}
		providers[name] = brain.ToolProvider{Parameters: json.RawMessage(`{"type":"object"}`), Authorize: authorize, Bind: func(_ context.Context, b brain.BindingContext) ([]brain.ManagedTool, error) {
			descriptors := service.Tools
			timeout := 300
			if name == "application" {
				var selected struct {
					Tools     []brain.Tool `json:"tools"`
					TimeoutMS int          `json:"timeout_ms"`
				}
				if err := json.Unmarshal(b.Parameters, &selected); err != nil {
					return nil, err
				}
				descriptors = selected.Tools
				timeout = (selected.TimeoutMS + 999) / 1000
			}
			if len(descriptors) == 0 {
				return nil, errors.New("installed provider requires descriptors")
			}
			var parameters map[string]json.RawMessage
			if err := json.Unmarshal(b.Parameters, &parameters); err != nil {
				return nil, err
			}
			grant, _ := json.Marshal(map[string]json.RawMessage{"authorization": parameters["authorization"]})
			payload, err := json.Marshal(capsule{authorization: authorization{Tenant: b.Tenant, Session: b.Session, Provider: name, Alias: b.Alias, Parameters: grant}, Credential: b.Credential})
			if err != nil {
				return nil, err
			}
			encoded := base64.RawURLEncoding.EncodeToString(payload)
			mac := hmac.New(sha256.New, []byte(toolToken))
			mac.Write([]byte(encoded))
			credential := encoded + "." + base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
			bindings := make([]brain.ManagedTool, 0, len(descriptors))
			for _, descriptor := range descriptors {
				bindings = append(bindings, brain.ManagedTool{Tool: descriptor, URL: strings.TrimRight(service.URL, "/") + "/" + name + "/" + descriptor.Name, Credential: credential, TimeoutSeconds: timeout, MaxOutputBytes: 2 << 20})
			}
			return bindings, nil
		}}
	}
	return providers, nil
}
