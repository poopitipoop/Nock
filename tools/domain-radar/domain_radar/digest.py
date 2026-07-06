"""Formats scored terms into the morning digest message."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

from .buyer_links import BuyerLeadLinks
from .scoring import ScoreBreakdown
from .storage import Mention


@dataclass
class DigestSignal:
    term: str
    category: Optional[str]
    est_lifespan: Optional[str]
    availability: dict[str, Optional[bool]]  # "term.tld" -> True/False/None(unknown)
    score: ScoreBreakdown
    mentions: list[Mention]
    buyer_links: BuyerLeadLinks


def _availability_lines(availability: dict[str, Optional[bool]]) -> list[str]:
    lines = []
    for domain, avail in availability.items():
        if avail is True:
            status = "free"
        elif avail is False:
            status = "taken -- check listing/registrar before assuming it's for sale"
        else:
            status = "unknown (WHOIS check failed, verify manually)"
        lines.append(f"  {domain}: {status}")
    return lines


def _recommendation(availability: dict[str, Optional[bool]]) -> str:
    free = [d for d, a in availability.items() if a is True]
    if not free:
        return "No configured TLD is free -- this window has likely already closed."
    # prefer .io/.ai/.dev over lesser-known TLDs when several are free
    preferred_order = ["io", "ai", "dev", "co", "com", "net", "app"]
    free.sort(key=lambda d: preferred_order.index(d.rsplit(".", 1)[1])
              if d.rsplit(".", 1)[1] in preferred_order else 99)
    best = free[0]
    return f"Register {best} now if the confidence score and mentions below hold up to a manual read."


def format_signal(signal: DigestSignal, index: int) -> str:
    platforms = sorted({m.platform for m in signal.mentions})
    origin = ", ".join(platforms) if platforms else "unknown"
    sample_urls = [m.source_url for m in signal.mentions[:3] if m.source_url]

    lines = [
        f'Signal #{index}: "{signal.term}"',
        f"Origin: {origin} ({len(signal.mentions)} mention(s) tracked)",
        f"Type: {signal.category or 'unclassified'}",
        f"Lifespan (LLM estimate, verify yourself): {signal.est_lifespan or 'unknown'}",
        "Domain availability:",
        *_availability_lines(signal.availability),
        "Buyer research (unverified -- click through and confirm before contacting anyone):",
        f"  X search: {signal.buyer_links.x_search}",
        f"  Crunchbase search: {signal.buyer_links.google_crunchbase_search}",
        f"  General search: {signal.buyer_links.google_general_search}",
        f"Confidence: {signal.score.total}/100",
        f"Recommendation: {_recommendation(signal.availability)}",
    ]
    if sample_urls:
        lines.append("Sources:")
        lines.extend(f"  {u}" for u in sample_urls)
    return "\n".join(lines)


def format_digest(signals: list[DigestSignal]) -> str:
    if not signals:
        return "No signals cleared the confidence threshold this run."
    parts = [format_signal(sig, i + 1) for i, sig in enumerate(signals)]
    return "\n\n".join(parts)
