// Outbound fetching for user-submitted URLs.
//
// This is the security-critical file. Anyone on the internet can hand this
// service a URL and make it issue a request, so without care it becomes an
// SSRF proxy into whatever network it runs in: cloud metadata endpoints,
// internal admin panels, databases on localhost. Guards applied here:
//
//   - only http/https
//   - hostname resolved up front, and every resolved address checked against
//     private/loopback/link-local ranges before the request is made
//   - redirects followed manually so each hop is re-validated (a public host
//     can redirect to 127.0.0.1 or 169.254.169.254)
//   - hard timeout and response size cap so one URL cannot hang or exhaust memory
//
// The resolve-then-check approach still has a TOCTOU gap in principle (DNS
// could change between our lookup and the request). Closing that fully means
// pinning the connection to the checked address via a custom agent; it is
// noted here as a known limitation rather than left implied.

import { lookup } from 'node:dns/promises';
import { config } from './config.js';

const MAX_REDIRECTS = 5;

export class FetchError extends Error {
  constructor(message, code) {
    super(message);
    this.name = 'FetchError';
    this.code = code;
  }
}

function ipv4IsPrivate(ip) {
  const parts = ip.split('.').map(Number);
  if (parts.length !== 4 || parts.some((p) => !Number.isInteger(p) || p < 0 || p > 255)) {
    return true; // Unparseable: refuse rather than guess.
  }
  const [a, b] = parts;
  if (a === 0 || a === 10 || a === 127) return true;
  if (a === 169 && b === 254) return true; // link-local, incl. cloud metadata
  if (a === 172 && b >= 16 && b <= 31) return true;
  if (a === 192 && b === 168) return true;
  if (a === 192 && b === 0) return true;
  if (a === 100 && b >= 64 && b <= 127) return true; // CGNAT
  if (a === 198 && (b === 18 || b === 19)) return true; // benchmarking
  if (a >= 224) return true; // multicast and reserved
  return false;
}

function ipv6IsPrivate(ip) {
  const addr = ip.toLowerCase().split('%')[0];
  if (addr === '::' || addr === '::1') return true;
  // IPv4-mapped (::ffff:a.b.c.d) inherits the IPv4 verdict.
  const mapped = addr.match(/^::ffff:(\d+\.\d+\.\d+\.\d+)$/);
  if (mapped) return ipv4IsPrivate(mapped[1]);
  const first = Number.parseInt(addr.split(':')[0] || '0', 16);
  if ((first & 0xfe00) === 0xfc00) return true; // fc00::/7 unique-local
  if ((first & 0xffc0) === 0xfe80) return true; // fe80::/10 link-local
  return false;
}

export function isPrivateAddress(ip, family) {
  return family === 6 ? ipv6IsPrivate(ip) : ipv4IsPrivate(ip);
}

/**
 * Accepts a user-typed URL and returns a normalized absolute URL, defaulting a
 * bare hostname to https.
 */
export function normalizeUrl(input) {
  const trimmed = String(input ?? '').trim();
  if (!trimmed) throw new FetchError('Enter a URL to scan.', 'empty');
  const withScheme = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(trimmed) ? trimmed : `https://${trimmed}`;
  let url;
  try {
    url = new URL(withScheme);
  } catch {
    throw new FetchError(`That does not look like a valid URL: ${trimmed}`, 'invalid');
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new FetchError('Only http and https URLs can be scanned.', 'scheme');
  }
  url.hash = '';
  return url;
}

/** Throws unless every address the hostname resolves to is publicly routable. */
export async function assertPublicHost(hostname) {
  let addresses;
  try {
    addresses = await lookup(hostname, { all: true });
  } catch {
    throw new FetchError(`Could not resolve ${hostname}.`, 'dns');
  }
  if (addresses.length === 0) {
    throw new FetchError(`Could not resolve ${hostname}.`, 'dns');
  }
  for (const { address, family } of addresses) {
    if (isPrivateAddress(address, family)) {
      throw new FetchError(
        `${hostname} resolves to a private address (${address}); only public sites can be scanned.`,
        'private',
      );
    }
  }
  return addresses;
}

async function readCapped(response, maxBytes) {
  if (!response.body) return '';
  const chunks = [];
  let total = 0;
  for await (const chunk of response.body) {
    total += chunk.length;
    if (total > maxBytes) {
      chunks.push(chunk.subarray(0, Math.max(0, chunk.length - (total - maxBytes))));
      break;
    }
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString('utf8');
}

/**
 * Fetches a URL with redirects validated at every hop.
 *
 * @returns {Promise<{url: string, status: number, headers: Headers, body: string,
 *                    elapsedMs: number, redirects: string[], truncated: boolean}>}
 */
export async function fetchPage(input, { maxBytes = config.maxResponseBytes } = {}) {
  let url = input instanceof URL ? input : normalizeUrl(input);
  const redirects = [];
  const startedAt = Date.now();

  for (let hop = 0; hop <= MAX_REDIRECTS; hop += 1) {
    await assertPublicHost(url.hostname);

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), config.requestTimeoutMs);
    let response;
    try {
      response = await fetch(url, {
        redirect: 'manual',
        signal: controller.signal,
        headers: {
          'user-agent': config.userAgent,
          accept: 'text/html,application/xhtml+xml,*/*;q=0.8',
          'accept-language': 'en',
        },
      });
    } catch (error) {
      if (error.name === 'AbortError') {
        throw new FetchError(
          `${url.hostname} did not respond within ${config.requestTimeoutMs / 1000}s.`,
          'timeout',
        );
      }
      throw new FetchError(`Could not reach ${url.hostname}: ${error.message}`, 'network');
    } finally {
      clearTimeout(timer);
    }

    const location = response.headers.get('location');
    if (response.status >= 300 && response.status < 400 && location) {
      if (hop === MAX_REDIRECTS) {
        throw new FetchError('Too many redirects.', 'redirects');
      }
      let next;
      try {
        next = new URL(location, url);
      } catch {
        throw new FetchError(`Invalid redirect target: ${location}`, 'invalid');
      }
      if (next.protocol !== 'http:' && next.protocol !== 'https:') {
        throw new FetchError(`Redirect to unsupported scheme: ${next.protocol}`, 'scheme');
      }
      redirects.push(url.toString());
      url = next;
      // Body is discarded; cancel so the socket is not left half-read.
      await response.body?.cancel().catch(() => {});
      continue;
    }

    const body = await readCapped(response, maxBytes);
    return {
      url: url.toString(),
      status: response.status,
      headers: response.headers,
      body,
      elapsedMs: Date.now() - startedAt,
      redirects,
      truncated: Buffer.byteLength(body, 'utf8') >= maxBytes,
    };
  }

  throw new FetchError('Too many redirects.', 'redirects');
}

/**
 * Fetches a same-origin support file (robots.txt, llms.txt). Absence is a
 * normal, expected answer, so failures resolve to null instead of throwing.
 */
export async function fetchSibling(baseUrl, path) {
  try {
    const target = new URL(path, baseUrl);
    const result = await fetchPage(target, { maxBytes: 256 * 1024 });
    if (result.status !== 200) return { found: false, status: result.status, body: null };
    return { found: true, status: result.status, body: result.body };
  } catch {
    return { found: false, status: null, body: null };
  }
}
