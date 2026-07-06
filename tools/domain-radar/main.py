#!/usr/bin/env python3
"""CLI entrypoint. Run hourly via cron/systemd-timer/GitHub Actions schedule.

    python main.py --config config.yaml
"""

from __future__ import annotations

import argparse
import logging

from domain_radar.config import load_config
from domain_radar.pipeline import run_once


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", default="config.yaml", help="path to config.yaml")
    parser.add_argument("--env", default=None, help="path to .env (defaults to ./.env)")
    parser.add_argument("-v", "--verbose", action="store_true")
    args = parser.parse_args()

    logging.basicConfig(level=logging.INFO if args.verbose else logging.WARNING,
                         format="%(asctime)s %(levelname)s %(name)s: %(message)s")

    cfg = load_config(args.config, args.env)
    digest_text = run_once(cfg)
    print(digest_text)


if __name__ == "__main__":
    main()
