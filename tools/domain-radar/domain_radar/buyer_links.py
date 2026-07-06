"""Builds ready-to-click search links for manual buyer research.

Deliberately does NOT ask the LLM to name specific companies "already using"
a term -- that's exactly the kind of claim a model can confidently
hallucinate, and putting a fabricated company name in front of a user who
might then cold-email them is a real harm. Instead we hand back pre-built
search queries so the human does the 30-second verification step themselves.
"""

from __future__ import annotations

from dataclasses import dataclass
from urllib.parse import quote_plus


@dataclass
class BuyerLeadLinks:
    x_search: str
    google_crunchbase_search: str
    google_general_search: str


def build_buyer_lead_links(term: str) -> BuyerLeadLinks:
    q = quote_plus(term)
    return BuyerLeadLinks(
        x_search=f"https://x.com/search?q={q}&f=live",
        google_crunchbase_search=f"https://www.google.com/search?q=site%3Acrunchbase.com+{q}",
        google_general_search=f"https://www.google.com/search?q=%22{q}%22+startup+OR+product",
    )
