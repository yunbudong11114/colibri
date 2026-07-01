from __future__ import annotations

import argparse
from pathlib import Path
from typing import Sequence

from colibri.config import AgentConfig
from colibri.model.fake import FakeModelClient
from colibri.session import AgentSession


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="colibri")
    parser.add_argument("--config", type=Path, default=None)
    subparsers = parser.add_subparsers(dest="command", required=True)

    ask = subparsers.add_parser("ask")
    ask.add_argument("text")

    subparsers.add_parser("repl")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    config = AgentConfig.load(args.config)
    session = AgentSession(config=config, model=FakeModelClient())

    if args.command == "ask":
        print(session.submit(args.text).text)
        return 0

    if args.command == "repl":
        return _run_repl(session)

    return 2


def _run_repl(session: AgentSession) -> int:
    while True:
        try:
            user_text = input("colibri> ")
        except EOFError:
            print()
            return 0

        if user_text.strip() in {"/quit", "/exit"}:
            return 0
        if not user_text.strip():
            continue

        print(session.submit(user_text).text)


if __name__ == "__main__":
    raise SystemExit(main())
