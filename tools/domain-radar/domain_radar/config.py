"""Loads config.yaml + .env into a single Config object."""

from __future__ import annotations

import os
from dataclasses import dataclass, field

import yaml
from dotenv import load_dotenv


@dataclass
class RedditConfig:
    subreddits: list[str]
    lookback_hours: int
    min_score: int
    client_id: str = ""
    client_secret: str = ""
    user_agent: str = "domain-radar/0.1"


@dataclass
class YoutubeConfig:
    channels: list[str]
    lookback_hours: int


@dataclass
class DomainsConfig:
    tlds: list[str]


@dataclass
class ScoringConfig:
    digest_threshold: int
    min_mentions: int


@dataclass
class DigestConfig:
    max_signals_per_run: int
    telegram_bot_token: str = ""
    telegram_chat_id: str = ""


@dataclass
class Config:
    reddit: RedditConfig
    youtube: YoutubeConfig
    domains: DomainsConfig
    scoring: ScoringConfig
    digest: DigestConfig
    anthropic_api_key: str = ""
    anthropic_model: str = "claude-sonnet-5"
    db_path: str = "domain_radar.sqlite3"


def load_config(config_path: str = "config.yaml", env_path: str | None = None) -> Config:
    load_dotenv(env_path) if env_path else load_dotenv()

    with open(config_path) as f:
        raw = yaml.safe_load(f)

    reddit = RedditConfig(
        subreddits=raw["reddit"]["subreddits"],
        lookback_hours=raw["reddit"]["lookback_hours"],
        min_score=raw["reddit"]["min_score"],
        client_id=os.environ.get("REDDIT_CLIENT_ID", ""),
        client_secret=os.environ.get("REDDIT_CLIENT_SECRET", ""),
        user_agent=os.environ.get("REDDIT_USER_AGENT", "domain-radar/0.1"),
    )
    youtube = YoutubeConfig(
        channels=raw["youtube"]["channels"],
        lookback_hours=raw["youtube"]["lookback_hours"],
    )
    domains = DomainsConfig(tlds=raw["domains"]["tlds"])
    scoring = ScoringConfig(
        digest_threshold=raw["scoring"]["digest_threshold"],
        min_mentions=raw["scoring"]["min_mentions"],
    )
    digest = DigestConfig(
        max_signals_per_run=raw["digest"]["max_signals_per_run"],
        telegram_bot_token=os.environ.get("TELEGRAM_BOT_TOKEN", ""),
        telegram_chat_id=os.environ.get("TELEGRAM_CHAT_ID", ""),
    )

    return Config(
        reddit=reddit,
        youtube=youtube,
        domains=domains,
        scoring=scoring,
        digest=digest,
        anthropic_api_key=os.environ.get("ANTHROPIC_API_KEY", ""),
        anthropic_model=os.environ.get("ANTHROPIC_MODEL", "claude-sonnet-5"),
        db_path=os.environ.get("DOMAIN_RADAR_DB", "domain_radar.sqlite3"),
    )
