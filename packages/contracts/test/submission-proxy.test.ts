import { describe, expect, it } from "vitest";
import { parseRunSubmissionRequest } from "../src/index.js";

const baseSubmission = {
  workspaceId: "workspace-1",
  idempotencyKey: "idem-1",
  submission: {
    model: "claude-haiku-4-5",
    prompt: ["say hello"],
    skills: [],
    agentsMd: [],
    files: [],
    mcpServers: []
  },
  secrets: {
    apiKey: "sk-ant-test"
  }
} as const;

const bearerPolicy = {
  name: "stripe",
  baseUrl: "https://api.stripe.com",
  authShape: { type: "bearer" as const },
  allowMethods: ["GET", "POST"],
  allowPathPrefixes: ["/v1/refunds", "/v1/charges"]
};

const bearerAuth = {
  name: "stripe",
  value: { type: "bearer" as const, token: "sk_live_xyz" }
};

describe("submission proxy endpoints — policy contract", () => {
  it("accepts a well-formed bearer endpoint with explicit auth", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      proxyEndpoints: [bearerPolicy],
      secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
    });
    expect(parsed.proxyEndpoints).toEqual([
      expect.objectContaining({
        name: "stripe",
        baseUrl: "https://api.stripe.com",
        authShape: { type: "bearer" },
        allowMethods: ["GET", "POST"],
        allowPathPrefixes: ["/v1/refunds", "/v1/charges"]
      })
    ]);
    expect(parsed.secrets.proxyEndpointAuth).toEqual([bearerAuth]);
  });

  it("accepts header, query, and basic auth shapes when policy and value agree", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      proxyEndpoints: [
        { ...bearerPolicy, name: "hdr", authShape: { type: "header", name: "X-Api-Key" } },
        { ...bearerPolicy, name: "qry", authShape: { type: "query", name: "api_key" } },
        { ...bearerPolicy, name: "bsc", authShape: { type: "basic" } }
      ],
      secrets: {
        ...baseSubmission.secrets,
        proxyEndpointAuth: [
          { name: "hdr", value: { type: "header", value: "abcdefgh" } },
          { name: "qry", value: { type: "query", value: "abcdefgh" } },
          { name: "bsc", value: { type: "basic", username: "u", password: "passwd-1234" } }
        ]
      }
    });
    expect(parsed.proxyEndpoints).toHaveLength(3);
  });

  it("normalizes baseUrl by stripping trailing slashes", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      proxyEndpoints: [{ ...bearerPolicy, baseUrl: "https://api.stripe.com/" }],
      secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
    });
    expect(parsed.proxyEndpoints?.[0]?.baseUrl).toBe("https://api.stripe.com");
  });

  it("rejects http:// baseUrl", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [{ ...bearerPolicy, baseUrl: "http://api.stripe.com" }],
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/https:\/\//);
  });

  it("rejects baseUrl with embedded credentials", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [{ ...bearerPolicy, baseUrl: "https://user:pass@api.stripe.com" }],
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/credentials/);
  });

  it("rejects reserved endpoint names", () => {
    for (const reserved of ["proxy", "aex", "internal", "admin"]) {
      expect(() =>
        parseRunSubmissionRequest({
          ...baseSubmission,
          proxyEndpoints: [{ ...bearerPolicy, name: reserved }],
          secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [{ ...bearerAuth, name: reserved }] }
        })
      ).toThrow(/reserved/);
    }
  });

  it("rejects endpoint names that violate the pattern", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [{ ...bearerPolicy, name: "Bad-Name" }],
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/pattern|name/i);
  });

  it("rejects duplicate endpoint names", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [bearerPolicy, bearerPolicy],
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/duplicate/);
  });

  it("rejects unknown HTTP methods", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [{ ...bearerPolicy, allowMethods: ["CONNECT"] }],
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/method/i);
  });

  it("rejects path prefixes containing traversal", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [{ ...bearerPolicy, allowPathPrefixes: ["/v1/../admin"] }],
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/traversal/);
  });

  it("rejects path prefixes that do not start with '/'", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [{ ...bearerPolicy, allowPathPrefixes: ["v1/refunds"] }],
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/\//);
  });

  it("rejects forbidden allow-list headers (denylist)", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [{ ...bearerPolicy, allowHeaders: ["Cookie"] }],
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/forbidden/);
  });

  it("rejects allow-list headers that collide with bearer auth carrier", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [{ ...bearerPolicy, allowHeaders: ["Authorization"] }],
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/forbidden|auth header/);
  });

  it("rejects allow-list headers that collide with header-auth carrier", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [
          {
            ...bearerPolicy,
            authShape: { type: "header", name: "X-Api-Key" },
            allowHeaders: ["x-api-key", "content-type"]
          }
        ],
        secrets: {
          ...baseSubmission.secrets,
          proxyEndpointAuth: [{ name: "stripe", value: { type: "header", value: "abc" } }]
        }
      })
    ).toThrow(/auth header/);
  });

  it("rejects negative or non-integer caps", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [{ ...bearerPolicy, maxRequestBytes: -1 }],
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/positive/);
  });

  it("rejects policy entry with no matching secrets.proxyEndpointAuth", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [bearerPolicy],
        secrets: { ...baseSubmission.secrets }
      })
    ).toThrow(/no matching secrets\.proxyEndpointAuth/);
  });

  it("rejects secrets.proxyEndpointAuth entry with no matching policy", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
      })
    ).toThrow(/no matching proxyEndpoints/);
  });

  it("rejects mismatched authShape.type vs value.type", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [bearerPolicy],
        secrets: {
          ...baseSubmission.secrets,
          proxyEndpointAuth: [{ name: "stripe", value: { type: "header", value: "abcdefgh" } }]
        }
      })
    ).toThrow(/value\.type must equal/);
  });

  it("rejects secret values shorter than the redactor minimum (8 bytes)", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [bearerPolicy],
        secrets: {
          ...baseSubmission.secrets,
          proxyEndpointAuth: [{ name: "stripe", value: { type: "bearer", token: "abc" } }]
        }
      })
    ).toThrow(/at least 8 bytes/);
  });

  it("rejects an inbound submission setting __aex_ namespaced secrets", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        secrets: {
          ...baseSubmission.secrets,
          __aex_proxy_token: "forged"
        } as never
      })
    ).toThrow(/__aex_/);
  });

  it("rejects unknown top-level secret keys", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        secrets: { ...baseSubmission.secrets, somethingElse: "x" } as never
      })
    ).toThrow(/not an allowed field/);
  });

  it("accepts a keyless (authShape:none) endpoint with no auth entry", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      proxyEndpoints: [
        {
          name: "wikimedia",
          baseUrl: "https://commons.wikimedia.org",
          authShape: { type: "none" },
          allowMethods: ["GET"],
          allowPathPrefixes: ["/wiki/", "/w/api.php"]
        }
      ]
    });
    expect(parsed.proxyEndpoints).toEqual([
      expect.objectContaining({
        name: "wikimedia",
        authShape: { type: "none" }
      })
    ]);
    expect(parsed.secrets.proxyEndpointAuth).toBeUndefined();
  });

  it("rejects a keyless endpoint that ships an auth entry", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [
          {
            name: "wikimedia",
            baseUrl: "https://commons.wikimedia.org",
            authShape: { type: "none" },
            allowMethods: ["GET"],
            allowPathPrefixes: ["/wiki/"]
          }
        ],
        secrets: {
          ...baseSubmission.secrets,
          proxyEndpointAuth: [
            { name: "wikimedia", value: { type: "bearer", token: "should-not-be-here" } }
          ]
        }
      })
    ).toThrow(/authShape "none".*remove the auth entry/);
  });

  it("rejects authShape:none with stray extra keys", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        proxyEndpoints: [
          {
            name: "wikimedia",
            baseUrl: "https://commons.wikimedia.org",
            authShape: { type: "none", name: "should-not-be-here" } as never,
            allowMethods: ["GET"],
            allowPathPrefixes: ["/wiki/"]
          }
        ]
      })
    ).toThrow(/not an allowed field/);
  });

  it("accepts mixed keyless + bearer endpoints in one submission", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      proxyEndpoints: [
        {
          name: "wikimedia",
          baseUrl: "https://commons.wikimedia.org",
          authShape: { type: "none" },
          allowMethods: ["GET"],
          allowPathPrefixes: ["/wiki/"]
        },
        bearerPolicy
      ],
      secrets: { ...baseSubmission.secrets, proxyEndpointAuth: [bearerAuth] }
    });
    expect(parsed.proxyEndpoints).toHaveLength(2);
    expect(parsed.secrets.proxyEndpointAuth).toEqual([bearerAuth]);
  });
});

describe("submission.environment — snapshot prerequisite for proxy worker", () => {
  it("accepts limited networking with allowedHosts", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        environment: {
          networking: { mode: "limited", allowedHosts: ["api.example.com"] }
        }
      }
    });
    expect(parsed.submission.environment?.networking).toEqual({
      mode: "limited",
      allowedHosts: ["api.example.com"]
    });
  });

  it("accepts packages array", () => {
    const parsed = parseRunSubmissionRequest({
      ...baseSubmission,
      submission: {
        ...baseSubmission.submission,
        environment: { packages: [{ name: "node", version: ">=20" }] }
      }
    });
    expect(parsed.submission.environment?.packages).toEqual([
      { name: "node", version: ">=20", ecosystem: "apt" }
    ]);
  });

  it("rejects unknown environment subfields", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          environment: { networking: { mode: "limited" }, secret: "x" } as never
        }
      })
    ).toThrow(/environment/);
  });

  it("rejects environment.networking without a mode", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          environment: { networking: { allowedHosts: ["a"] } } as never
        }
      })
    ).toThrow(/mode/);
  });

  it("rejects duplicate allowedHosts", () => {
    expect(() =>
      parseRunSubmissionRequest({
        ...baseSubmission,
        submission: {
          ...baseSubmission.submission,
          environment: { networking: { mode: "limited", allowedHosts: ["a", "A"] } }
        }
      })
    ).toThrow(/duplicate/);
  });
});
