from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path

from knowledge.llm import LlmError


@dataclass(frozen=True)
class LlmCall:
    system: str
    message: str
    add_dirs: tuple[Path, ...]


class FakeLlm:
    """The `Llm` of the tests: `responder(message)` is the text the model answers, or `error` is raised."""

    def __init__(self, responder: Callable[[str], str] | None = None, error: LlmError | None = None) -> None:
        self.responder = responder or _empty_answer
        self.error = error
        self.calls: list[LlmCall] = []

    def run(self, system: str, message: str, add_dirs: Sequence[Path]) -> str:
        self.calls.append(LlmCall(system, message, tuple(add_dirs)))
        if self.error is not None:
            raise self.error
        return self.responder(message)


def _empty_answer(message: str) -> str:
    return ""
