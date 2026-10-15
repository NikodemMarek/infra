"""The prompts of the package (`prompts/*.md`)."""

from importlib import resources


def read(name: str) -> str:
    """The text of `prompts/<name>`."""
    return resources.files("knowledge").joinpath("prompts", name).read_text(encoding="utf-8")
