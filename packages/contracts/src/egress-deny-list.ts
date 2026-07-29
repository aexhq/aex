/**
 * Public egress address policy shared by request admission and self-hosted
 * enforcement. The CIDR table owns both classification and downstream range
 * generation so consumers never need a hand-maintained copy.
 */

/** One private or unroutable address range refused by Aex. */
export interface EgressDeniedRange {
  /** Canonical CIDR used for generated network policy. */
  readonly cidr: string;
  readonly family: 4 | 6;
  /** The verbatim public rejection reason. */
  readonly reason: string;
  /** Whether network enforcement should deny the CIDR in addition to admission. */
  readonly denyRange: boolean;
  /** IPv6 textual matcher; IPv4 entries match numerically from {@link cidr}. */
  readonly match?: (host: string) => boolean;
}

/**
 * The canonical private and unroutable address set.
 *
 * IPv4 entries are classified numerically from `cidr`. IPv6 entries keep a
 * textual matcher beside the range because they are either exact special forms
 * or prefix families.
 */
export const EGRESS_DENIED_RANGES: readonly EgressDeniedRange[] = [
  {
    cidr: "0.0.0.0/8",
    family: 4,
    denyRange: true,
    reason: "must not target unroutable IPv4 (0.0.0.0/8)"
  },
  {
    cidr: "10.0.0.0/8",
    family: 4,
    denyRange: true,
    reason: "must not target RFC1918 IPv4 (10.0.0.0/8)"
  },
  {
    cidr: "127.0.0.0/8",
    family: 4,
    denyRange: true,
    reason: "must not target loopback IPv4 (127.0.0.0/8)"
  },
  {
    cidr: "169.254.0.0/16",
    family: 4,
    denyRange: true,
    reason: "must not target link-local IPv4 (169.254.0.0/16) — cloud metadata range"
  },
  {
    cidr: "100.64.0.0/10",
    family: 4,
    denyRange: true,
    reason: "must not target CGNAT IPv4 (100.64.0.0/10)"
  },
  {
    cidr: "198.18.0.0/15",
    family: 4,
    denyRange: true,
    reason: "must not target benchmark IPv4 (198.18.0.0/15)"
  },
  {
    cidr: "172.16.0.0/12",
    family: 4,
    denyRange: true,
    reason: "must not target RFC1918 IPv4 (172.16.0.0/12)"
  },
  {
    cidr: "192.168.0.0/16",
    family: 4,
    denyRange: true,
    reason: "must not target RFC1918 IPv4 (192.168.0.0/16)"
  },
  {
    cidr: "224.0.0.0/3",
    family: 4,
    denyRange: true,
    reason: "must not target multicast/reserved IPv4 (224.0.0.0/3)"
  },
  {
    cidr: "::/128",
    family: 6,
    denyRange: true,
    reason: "must not target unspecified IPv6 (::)",
    match: (host) => host === "::"
  },
  {
    cidr: "::1/128",
    family: 6,
    denyRange: true,
    reason: "must not target loopback IPv6 (::1)",
    match: (host) => host === "::1" || host === "0:0:0:0:0:0:0:1"
  },
  {
    cidr: "fe80::/10",
    family: 6,
    denyRange: true,
    reason: "must not target link-local IPv6 (fe80::/10)",
    match: (host) => /^fe[89ab][0-9a-f]?:/.test(host)
  },
  {
    cidr: "fc00::/7",
    family: 6,
    denyRange: true,
    reason: "must not target unique-local IPv6 (fc00::/7)",
    match: (host) => /^f[cd][0-9a-f]{0,2}:/.test(host)
  },
  {
    cidr: "64:ff9b::/96",
    family: 6,
    denyRange: true,
    reason: "must not target IPv6 NAT64 prefix (64:ff9b::/96)",
    match: (host) => host.startsWith("64:ff9b::")
  }
];

// `::ffff:0:0/96` is deliberately absent. Go's `net.IPNet.Contains`
// degrades an IPv4-mapped `/96` to IPv4 `0.0.0.0/0`, so enforcing it as a
// range would block all IPv4. It is unnecessary here: mapped literals are
// folded to IPv4 and classified by the IPv4 entries above.

/** IPv4 dotted-quad to uint32, or null when `host` is not a dotted-quad. */
function ipv4ToUint32(host: string): number | null {
  if (!/^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host)) return null;
  const octets = host.split(".").map((octet) => Number.parseInt(octet, 10));
  if (octets.some((octet) => octet > 255)) return null;
  return ((octets[0]! << 24) | (octets[1]! << 16) | (octets[2]! << 8) | octets[3]!) >>> 0;
}

/** Whether `value` falls inside the IPv4 `cidr`. */
function ipv4InCidr(value: number, cidr: string): boolean {
  const [network, prefixText] = cidr.split("/") as [string, string];
  const base = ipv4ToUint32(network);
  if (base === null) return false;
  const prefix = Number.parseInt(prefixText, 10);
  const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0;
  return (value & mask) === (base & mask);
}

/**
 * Return why an IPv4 literal must be refused, or null when it is not a
 * dotted-quad or is publicly routable.
 */
export function denyReasonForV4(host: string): string | null {
  const value = ipv4ToUint32(host);
  if (value === null) return null;
  for (const range of EGRESS_DENIED_RANGES) {
    if (range.family === 4 && ipv4InCidr(value, range.cidr)) return range.reason;
  }
  return null;
}

/**
 * Return why an IP-literal host must be refused, or null when it is publicly
 * routable or not an IP literal. `host` must already be lowercased and have
 * IPv6 brackets removed.
 *
 * This does not resolve hostnames. Callers that connect to named hosts still
 * need resolve-then-pin protection against DNS rebinding.
 */
export function denyReasonForHostIp(host: string): string | null {
  const mappedDotted = /^::(?:ffff:)?(\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3})$/.exec(host);
  if (mappedDotted) {
    return denyReasonForV4(mappedDotted[1]!);
  }

  const mappedHex = /^::ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/.exec(host);
  if (mappedHex) {
    const high = Number.parseInt(mappedHex[1]!, 16);
    const low = Number.parseInt(mappedHex[2]!, 16);
    return denyReasonForV4(`${high >> 8}.${high & 0xff}.${low >> 8}.${low & 0xff}`);
  }

  const ipv4Denial = denyReasonForV4(host);
  if (ipv4Denial) return ipv4Denial;

  for (const range of EGRESS_DENIED_RANGES) {
    if (range.family === 6 && range.match?.(host) === true) return range.reason;
  }
  return null;
}

/**
 * Return why an MCP server URL must be refused, or null when it is acceptable.
 */
export function denyReasonForMcpHost(parsed: URL): string | null {
  const host = parsed.hostname.toLowerCase().replace(/^\[|\]$/g, "");
  if (host === "localhost" || host.endsWith(".localhost")) {
    return "must not target a loopback hostname";
  }

  const ipDenial = denyReasonForHostIp(host);
  if (ipDenial) return ipDenial;

  if (parsed.protocol === "https:" && parsed.port !== "" && parsed.port !== "443") {
    return `must use port 443 for https (got ${parsed.port})`;
  }
  return null;
}
