from pathlib import Path

import pytest

from fakes import FakeLlm
from knowledge import ask
from knowledge.llm import LlmError
from knowledge.prompt import librarian_prompt
from knowledge.scopes import load_scopes
from knowledge.store import Store
from test_cli_submit import Runner


@pytest.fixture
def repo(memory_repo: Path, tmp_path: Path) -> Path:
    """The memory has a project `app` whose repo is under tmp_path."""
    project = memory_repo / "projects" / "app"
    project.mkdir(parents=True)
    (project / "overview.md").write_text("# app\n")
    path = tmp_path / "p" / "app"
    (memory_repo / "INDEX.md").write_text(f"| slug | status | repo |\n|---|---|---|\n| app | active | {path} |\n")
    return path


@pytest.fixture
def run(monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> Runner:
    return Runner(monkeypatch, capsys)


def fake_llm(monkeypatch: pytest.MonkeyPatch, fake: FakeLlm) -> FakeLlm:
    monkeypatch.setattr("knowledge.ask.ClaudeCli", lambda **kwargs: fake)
    return fake


# message


def test_message_has_the_question_the_cwd_the_project_and_the_scopes(memory_repo: Path, repo: Path):
    cwd = repo.parent / "app.agents" / "ws" / "src"  # a jj workspace of the project
    text = ask.message("what did we decide?", cwd, Store(memory_repo), load_scopes(memory_repo))
    assert text.splitlines() == [
        "Question: what did we decide?",
        "",
        f"Asked from: {repo / 'src'}",
        f"Project there: app (projects/app/, repo {repo})",
        "Scopes there: projects/app/facts, global",
    ]


def test_message_outside_known_repos_has_no_project(memory_repo: Path, repo: Path, tmp_path: Path):
    text = ask.message("q", tmp_path / "elsewhere", Store(memory_repo), load_scopes(memory_repo))
    assert "Project there: none in INDEX.md" in text
    assert "Scopes there: global" in text.splitlines()


# prompt


def test_librarian_prompt_is_filled(memory_repo: Path):
    text = librarian_prompt(memory_repo)
    assert "@@" not in text
    assert str(memory_repo) in text


# answer


def test_answer_is_stripped_and_the_call_has_no_extra_dirs(memory_repo: Path, repo: Path):
    fake = FakeLlm(lambda message: "\n  Use --into.\nSources: topic/jj/absorb.md\n\n")
    assert ask.answer("how to absorb?", repo, fake) == "Use --into.\nSources: topic/jj/absorb.md"
    (call,) = fake.calls
    assert call.add_dirs == ()
    assert call.system == librarian_prompt(memory_repo)
    assert call.message.startswith("Question: how to absorb?\n")


def test_answer_uses_the_sonnet_low_runner_by_default(memory_repo: Path, monkeypatch: pytest.MonkeyPatch):
    seen: list[dict[str, object]] = []

    def factory(**kwargs: object) -> FakeLlm:
        seen.append(kwargs)
        return FakeLlm(lambda message: "Not in memory.")

    monkeypatch.setattr("knowledge.ask.ClaudeCli", factory)
    assert ask.answer("q", memory_repo) == "Not in memory."
    assert seen == [{"model": "sonnet", "effort": "low", "timeout": 110}]


def test_an_empty_answer_is_an_error(memory_repo: Path):
    with pytest.raises(LlmError, match="empty answer"):
        ask.answer("q", memory_repo, FakeLlm(lambda message: " \n"))


# the command


def test_ask_joins_the_arguments(run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path):
    fake = fake_llm(monkeypatch, FakeLlm(lambda message: "an answer\n"))
    code, out = run("ask", "what", "is", "known?", stdin="ignored")
    assert (code, out) == (0, "an answer\n")
    assert fake.calls[0].message.startswith("Question: what is known?\n")


def test_ask_reads_stdin_without_arguments(run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path):
    fake = fake_llm(monkeypatch, FakeLlm(lambda message: "an answer"))
    code, out = run("ask", stdin="what is known?\n")
    assert (code, out) == (0, "an answer\n")
    assert fake.calls[0].message.startswith("Question: what is known?\n")


def test_ask_without_a_question_is_a_usage_error(run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path):
    fake = fake_llm(monkeypatch, FakeLlm())
    assert run("ask", stdin=" \n") == (2, "")
    assert run("ask", " ", stdin="") == (2, "")
    assert fake.calls == []


def test_ask_without_a_memory_dir_is_a_usage_error(run: Runner, monkeypatch: pytest.MonkeyPatch):
    fake = fake_llm(monkeypatch, FakeLlm())
    assert run("ask", "q") == (2, "")
    assert fake.calls == []


def test_ask_when_the_call_fails_exits_1(
    run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path, caplog: pytest.LogCaptureFixture
):
    fake_llm(monkeypatch, FakeLlm(error=LlmError("claude timed out after 300s")))
    assert run("ask", "q") == (1, "")
    assert "claude timed out after 300s" in caplog.text
