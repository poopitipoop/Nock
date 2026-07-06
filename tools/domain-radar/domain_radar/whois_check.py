"""Batch WHOIS availability checks across a configurable list of TLDs.

Talks directly to each registry's WHOIS server on port 43 rather than pulling
in a heavier third-party WHOIS library, since all we need is an availability
signal, not full record parsing. Rate-limited to be a polite client -- WHOIS
servers commonly throttle or block bursty callers.
"""

from __future__ import annotations

import socket
import time
from typing import Callable, Optional

WHOIS_SERVERS = {
    "com": "whois.verisign-grs.com",
    "net": "whois.verisign-grs.com",
    "io": "whois.nic.io",
    "ai": "whois.nic.ai",
    "co": "whois.nic.co",
    "dev": "whois.nic.google",
    "app": "whois.nic.google",
}

# Substrings registries commonly use to say "nobody owns this yet".
# Best-effort: WHOIS response formats aren't standardized across registries,
# so treat results as a strong hint, not ground truth -- always double check
# an "available" result before paying to register it.
NOT_FOUND_MARKERS = [
    "no match for",
    "not found",
    "no data found",
    "no entries found",
    "is available for registration",
    "domain not found",
    "no object found",
    "status: free",
]

QueryFn = Callable[[str, str, float], str]


def _raw_whois_query(server: str, query: str, timeout: float = 10.0) -> str:
    with socket.create_connection((server, 43), timeout=timeout) as sock:
        sock.sendall((query + "\r\n").encode())
        chunks = []
        while True:
            data = sock.recv(4096)
            if not data:
                break
            chunks.append(data)
    return b"".join(chunks).decode(errors="replace")


def check_domain(slug: str, tld: str, *, query_fn: QueryFn = _raw_whois_query) -> Optional[bool]:
    """Returns True if available, False if taken, None if the check itself failed."""
    server = WHOIS_SERVERS.get(tld)
    if server is None:
        raise ValueError(f"no WHOIS server configured for TLD {tld!r}")
    domain = f"{slug}.{tld}"
    try:
        text = query_fn(server, domain, 10.0)
    except OSError:
        return None
    lowered = text.lower()
    return any(marker in lowered for marker in NOT_FOUND_MARKERS)


def check_all_tlds(slug: str, tlds: list[str], *, query_fn: QueryFn = _raw_whois_query,
                    rate_limit_seconds: float = 1.0) -> dict[str, Optional[bool]]:
    results: dict[str, Optional[bool]] = {}
    for i, tld in enumerate(tlds):
        if i > 0:
            time.sleep(rate_limit_seconds)
        results[f"{slug}.{tld}"] = check_domain(slug, tld, query_fn=query_fn)
    return results
