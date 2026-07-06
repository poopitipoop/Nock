"""Pulls recent posts+comments from configured subreddits via the official read-only API."""

from __future__ import annotations

import time
from dataclasses import dataclass

import praw

from ..config import RedditConfig


@dataclass
class RawItem:
    platform: str
    text: str
    url: str
    engagement: int
    seen_at: float


def _read_only_client(cfg: RedditConfig) -> praw.Reddit:
    return praw.Reddit(
        client_id=cfg.client_id,
        client_secret=cfg.client_secret,
        user_agent=cfg.user_agent,
    )


def fetch_recent(cfg: RedditConfig, client: praw.Reddit | None = None) -> list[RawItem]:
    """Fetch new submissions (title + selftext) from each configured subreddit,
    filtered to the lookback window and the minimum score threshold.
    """
    reddit = client or _read_only_client(cfg)
    cutoff = time.time() - cfg.lookback_hours * 3600
    items: list[RawItem] = []

    for name in cfg.subreddits:
        subreddit = reddit.subreddit(name)
        for post in subreddit.new(limit=100):
            if post.created_utc < cutoff:
                break
            if post.score < cfg.min_score:
                continue
            text = post.title if not post.selftext else f"{post.title}\n{post.selftext}"
            items.append(
                RawItem(
                    platform="reddit",
                    text=text[:2000],
                    url=f"https://reddit.com{post.permalink}",
                    engagement=post.score,
                    seen_at=post.created_utc,
                )
            )
    return items
