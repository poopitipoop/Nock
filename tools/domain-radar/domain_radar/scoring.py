"""Turns raw mentions + metadata into a single 0-100 confidence score.

Six signals, weighted to sum to 100:
  1. Mention velocity      (20) -- is mention rate accelerating right now?
  2. Cross-platform spread (15) -- seen on more than one source type?
  3. Category weight        (20) -- LLM-classified as a real category/pattern vs. a meme
  4. Engagement quality     (15) -- are the mentions themselves getting traction?
  5. Mention breadth        (15) -- how many independent mentions corroborate this?
  6. Domain opportunity     (15) -- is there still room to register something useful?

This is a heuristic, not a statistical model -- thresholds below are
deliberately simple and meant to be tuned once you see real digests.
"""

from __future__ import annotations

import math
import time
from dataclasses import dataclass
from typing import Optional

from .storage import Mention

CATEGORY_POINTS = {
    "technology_category": 20,
    "product_pattern": 16,
    "slang": 8,
    "meme": 4,
    "other": 2,
}

DAY = 86400.0


@dataclass
class ScoreBreakdown:
    velocity: float
    cross_platform: float
    category: float
    engagement: float
    breadth: float
    domain_opportunity: float

    @property
    def total(self) -> int:
        raw = (self.velocity + self.cross_platform + self.category
               + self.engagement + self.breadth + self.domain_opportunity)
        return max(0, min(100, round(raw)))


def _velocity_score(mentions: list[Mention], now: float) -> float:
    recent = sum(1 for m in mentions if now - m.seen_at <= DAY)
    prior = sum(1 for m in mentions if DAY < now - m.seen_at <= 2 * DAY)
    if recent == 0:
        return 0.0
    ratio = (recent - prior) / max(prior, 1)
    return min(max(ratio, 0.0) / 3.0, 1.0) * 20


def _cross_platform_score(mentions: list[Mention]) -> float:
    platforms = {m.platform for m in mentions}
    return 15.0 * min(len(platforms), 2) / 2


def _category_score(category: Optional[str]) -> float:
    return CATEGORY_POINTS.get(category or "other", 5)


def _engagement_score(mentions: list[Mention]) -> float:
    if not mentions:
        return 0.0
    avg = sum(m.engagement for m in mentions) / len(mentions)
    # log-scaled: reddit scores are heavy-tailed, a handful of viral mentions
    # shouldn't single-handedly max this out.
    return min(math.log1p(avg) / math.log1p(200), 1.0) * 15


def _breadth_score(mentions: list[Mention]) -> float:
    return min(len(mentions) / 6.0, 1.0) * 15


def _domain_opportunity_score(availability: dict[str, Optional[bool]]) -> float:
    known = [v for v in availability.values() if v is not None]
    if not known:
        return 7.5  # unknown -- neutral, don't let a WHOIS outage tank the score
    available_count = sum(1 for v in known if v)
    if available_count == 0:
        return 0.0  # window's closed, everything worth having is gone
    return min(available_count, 3) / 3.0 * 15


def score_term(mentions: list[Mention], category: Optional[str],
                availability: dict[str, Optional[bool]], *, now: Optional[float] = None) -> ScoreBreakdown:
    now = now if now is not None else time.time()
    return ScoreBreakdown(
        velocity=_velocity_score(mentions, now),
        cross_platform=_cross_platform_score(mentions),
        category=_category_score(category),
        engagement=_engagement_score(mentions),
        breadth=_breadth_score(mentions),
        domain_opportunity=_domain_opportunity_score(availability),
    )
