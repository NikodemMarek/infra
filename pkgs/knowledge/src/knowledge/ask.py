"""`knowledge ask`: one read-only model call over the memory store that answers a question with sources."""

from pathlib import Path

from .config import memory_dir
from .llm import ClaudeCli, Llm, LlmError
from .prompt import librarian_prompt
from .scopes import ScopesConfig, load_scopes, normalise_cwd, project_for_cwd, scopes_for_cwd
from .store import Store

MODEL = "sonnet"
EFFORT = "low"
TIMEOUT = 110.0  # seconds; below the Bash tool's 2 min default, so agents see this error, not a tool timeout


def message(question: str, cwd: Path, store: Store, cfg: ScopesConfig) -> str:
    """What is sent on stdin: the question and where it was asked from (the librarian searches there first)."""
    projects = store.projects_index()
    row = project_for_cwd(cwd, projects)
    project = f"{row.slug} (projects/{row.slug}/, repo {row.repo})" if row is not None else "none in INDEX.md"
    scopes = scopes_for_cwd(cwd, cfg, projects, store.project_slugs())
    return "\n".join(
        [
            f"Question: {question}",
            "",
            f"Asked from: {normalise_cwd(cwd)}",
            f"Project there: {project}",
            f"Scopes there: {', '.join(scopes)}",
            "",
        ]
    )


def answer(question: str, cwd: Path, llm: Llm | None = None) -> str:
    """The librarian's answer. Raises LlmError if the call fails."""
    memory = memory_dir()
    msg = message(question, cwd, Store(memory), load_scopes(memory))
    llm = llm or ClaudeCli(model=MODEL, effort=EFFORT, timeout=TIMEOUT)
    text = llm.run(librarian_prompt(memory), msg, []).strip()
    if not text:
        raise LlmError("claude returned an empty answer")
    return text
