package main

import (
	"net/http/httptest"
	"strings"
	"testing"
)

func TestGatewayAuthorityRequiresServiceCredentialAndTenant(t *testing.T) {
	token := strings.Repeat("a", 32)
	auth := authority(token)
	for _, test := range []struct {
		token, tenant, session string
		allowed                bool
	}{
		{"", "a", "", false}, {"customer", "a", "", false},
		{token, "", "", false}, {token, " a", "", false},
		{token, "a", "", true}, {token, "a", strings.Repeat("b", 32), true},
	} {
		request := httptest.NewRequest("GET", "http://127.0.0.1/sessions", nil)
		request.Header.Set("Authorization", "Bearer "+test.token)
		request.Header.Set("X-Aex-Tenant", test.tenant)
		request.Header.Set("X-Aex-Session", test.session)
		grant, ok := auth(request)
		if ok != test.allowed {
			t.Fatalf("unexpected authority: %v", ok)
		}
		if ok && (grant.Tenant != test.tenant || grant.Session != test.session || !grant.Manage) {
			t.Fatal("scope was widened")
		}
	}
}
