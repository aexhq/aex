import { describe, expect, it } from "bun:test";
import {
  EGRESS_DENIED_RANGES,
  denyReasonForHostIp,
  denyReasonForMcpHost,
  denyReasonForV4,
  type EgressDeniedRange
} from "../src/index.js";

const EXPECTED_CIDRS = [
  "0.0.0.0/8",
  "10.0.0.0/8",
  "127.0.0.0/8",
  "169.254.0.0/16",
  "100.64.0.0/10",
  "198.18.0.0/15",
  "172.16.0.0/12",
  "192.168.0.0/16",
  "224.0.0.0/3",
  "::/128",
  "::1/128",
  "fe80::/10",
  "fc00::/7",
  "64:ff9b::/96"
] as const;

describe("the public egress deny-list contract", () => {
  it("exports the complete ordered CIDR table for downstream enforcement", () => {
    const ranges: readonly EgressDeniedRange[] = EGRESS_DENIED_RANGES;

    expect(ranges.map((range) => range.cidr)).toEqual([...EXPECTED_CIDRS]);
    expect(new Set(ranges.map((range) => range.cidr)).size).toBe(ranges.length);
    expect(ranges.every((range) => range.denyRange)).toBe(true);
    expect(ranges.every((range) => range.reason.length > 0)).toBe(true);
  });

  it.each([
    ["0.0.0.0", /unroutable IPv4/i],
    ["10.0.0.1", /RFC1918 IPv4/i],
    ["127.0.0.1", /loopback IPv4/i],
    ["169.254.169.254", /link-local IPv4/i],
    ["100.64.0.1", /CGNAT IPv4/i],
    ["198.18.0.1", /benchmark IPv4/i],
    ["172.16.0.1", /RFC1918 IPv4/i],
    ["192.168.0.1", /RFC1918 IPv4/i],
    ["224.0.0.1", /multicast\/reserved IPv4/i]
  ])("classifies denied IPv4 %s from that table", (host, reason) => {
    expect(denyReasonForV4(host)).toMatch(reason);
    expect(denyReasonForHostIp(host)).toMatch(reason);
  });

  it("classifies IPv6 and mapped IPv4 without a second range list", () => {
    expect(denyReasonForHostIp("::1")).toMatch(/loopback IPv6/i);
    expect(denyReasonForHostIp("fc00::1")).toMatch(/unique-local IPv6/i);
    expect(denyReasonForHostIp("64:ff9b::1")).toMatch(/NAT64/i);
    expect(denyReasonForHostIp("::ffff:7f00:1")).toMatch(/loopback IPv4/i);
    expect(denyReasonForHostIp("2606:4700::1111")).toBeNull();
  });

  it("exports the URL classifier consumed by MCP admission", () => {
    expect(denyReasonForMcpHost(new URL("http://localhost/mcp"))).toMatch(/loopback hostname/i);
    expect(denyReasonForMcpHost(new URL("https://example.com:8443/mcp"))).toMatch(/port 443/i);
    expect(denyReasonForMcpHost(new URL("https://example.com/mcp"))).toBeNull();
  });
});
