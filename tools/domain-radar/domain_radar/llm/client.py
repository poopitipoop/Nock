"""Thin wrapper around the Anthropic SDK, forcing structured JSON via tool-use."""

from __future__ import annotations

from typing import Any

import anthropic


class LLMClient:
    def __init__(self, api_key: str, model: str = "claude-sonnet-5"):
        self._client = anthropic.Anthropic(api_key=api_key)
        self.model = model

    def call_tool(self, *, system: str, user: str, tool_name: str,
                   tool_description: str, input_schema: dict[str, Any],
                   max_tokens: int = 2000) -> dict[str, Any]:
        """Send a single-turn message and force the model to respond via the
        given tool, returning its parsed input. Raises if the model doesn't
        use the tool.
        """
        response = self._client.messages.create(
            model=self.model,
            max_tokens=max_tokens,
            system=system,
            messages=[{"role": "user", "content": user}],
            tools=[{
                "name": tool_name,
                "description": tool_description,
                "input_schema": input_schema,
            }],
            tool_choice={"type": "tool", "name": tool_name},
        )
        for block in response.content:
            if block.type == "tool_use" and block.name == tool_name:
                return block.input
        raise RuntimeError(f"model did not call tool {tool_name!r}")
