from pathlib import Path

import pytest

from fakes import FakeLlm
from knowledge import route
from knowledge.llm import LlmError
from knowledge.prompt import router_prompt
from test_import import Runner


def make_repo(root: Path, name: str, readme: str | None = None, readme_name: str = "README.md") -> Path:
    path = root / name
    path.mkdir(parents=True)
    if readme is not None:
        (path / readme_name).write_text(readme)
    return path


@pytest.fixture
def run(monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> Runner:
    return Runner(monkeypatch, capsys)


def fake_llm(monkeypatch: pytest.MonkeyPatch, fake: FakeLlm) -> FakeLlm:
    monkeypatch.setattr("knowledge.route.ClaudeCli", lambda **kwargs: fake)
    return fake


def answering(repo: str | None, reason: str = "because") -> FakeLlm:
    shown = "null" if repo is None else f'"{repo}"'
    return FakeLlm(lambda message: f'{{"repo": {shown}, "reason": "{reason}"}}')


# README


def test_readme_head_skips_headings_badges_html_fences_and_underlines(tmp_path: Path):
    readme = (
        "# Title\n"
        "\n"
        "[![ci](a.svg)](b)\n"
        "![logo](c.png)\n"
        '<p align="center">logo</p>\n'
        "\n"
        "Title\n"
        "=====\n"
        "\n"
        "```sh\n"
        "make install\n"
        "```\n"
        "A billing\n"
        "service.\n"
        "\n"
        "Sub\n"
        "---\n"
        "~~~\n"
        "more code\n"
        "~~~\n"
        "Second paragraph.\n"
    )
    repo = make_repo(tmp_path, "app", readme)
    assert route.readme_head(repo) == "Title A billing service. Sub Second paragraph."


def test_readme_head_takes_the_first_readme_in_any_case(tmp_path: Path):
    repo = make_repo(tmp_path, "app", "plain text\n", readme_name="readme.rst")
    (repo / "Readme.md").write_text("other\n")
    assert route.readme_head(repo) == "other"


def test_readme_head_prefers_the_shortest_name_to_a_translation(tmp_path: Path):
    repo = make_repo(tmp_path, "app", "English\n")
    (repo / "README-zh.md").write_text("Chinese\n")
    (repo / "README.de.md").write_text("German\n")
    assert route.readme_head(repo) == "English"


def test_readme_head_without_a_readme_or_text_is_empty(tmp_path: Path):
    assert route.readme_head(make_repo(tmp_path, "none")) == ""
    assert route.readme_head(make_repo(tmp_path, "headings", "# Only\n\n[![b](c)](d)\n")) == ""
    assert route.readme_head(tmp_path / "missing") == ""


def test_readme_head_is_truncated(tmp_path: Path):
    repo = make_repo(tmp_path, "app", "word " * 500)
    text = route.readme_head(repo)
    assert text.endswith("...")
    assert len(text) <= route.DESCRIPTION_CHARS


def test_readme_head_reads_only_the_start(tmp_path: Path):
    repo = make_repo(tmp_path, "app", "ok\n" + "x" * 20000)
    assert route.readme_head(repo).startswith("ok xxx")


# description


@pytest.fixture
def project(memory_repo: Path, tmp_path: Path) -> Path:
    """The memory has a project `app` (with a Purpose) whose repo has a README of its own."""
    app = make_repo(tmp_path / "p", "app", "# app\n\nThe README text.\n")
    overview = memory_repo / "projects" / "app" / "overview.md"
    overview.parent.mkdir(parents=True)
    overview.write_text("# app\n\n## Purpose\n\nBilling\nfor all.\n\n## Status\n\nactive\n")
    (memory_repo / "INDEX.md").write_text(f"| slug | status | repo |\n|---|---|---|\n| app | active | {app} |\n")
    return app


def test_purpose_is_the_section_of_the_overview(memory_repo: Path, project: Path):
    assert route.purpose(memory_repo, "app") == "Billing for all."
    assert route.purpose(memory_repo, "other") == ""


def test_purpose_is_empty_without_the_section(memory_repo: Path, project: Path):
    (memory_repo / "projects" / "app" / "overview.md").write_text("# app\n\n## Status\n\nactive\n")
    assert route.purpose(memory_repo, "app") == ""


def test_the_purpose_is_preferred_to_the_readme(memory_repo: Path, project: Path):
    text = route.message("t", [project], memory_repo)
    assert text.splitlines()[-1] == "- app: Billing for all."


def test_the_readme_is_used_without_a_purpose(memory_repo: Path, project: Path, tmp_path: Path):
    (memory_repo / "projects" / "app" / "overview.md").write_text("# app\n")
    assert route.message("t", [project], memory_repo).splitlines()[-1] == "- app: The README text."
    lib = make_repo(tmp_path / "p", "lib", "A library.\n")  # not in INDEX.md
    assert route.message("t", [lib], memory_repo).splitlines()[-1] == "- lib: A library."


# message


def test_message_lists_every_candidate_in_order_with_the_task_verbatim(memory_repo: Path, tmp_path: Path):
    zeta = make_repo(tmp_path, "zeta", "Zeta things.\n")
    bare = make_repo(tmp_path, "bare")
    alpha = make_repo(tmp_path, "alpha", "Alpha things.\n")
    text = route.message("fix  the\n  bug\n", [zeta, bare, alpha], memory_repo)
    assert text == (
        "Task:\nfix  the\n  bug\n\n\n"
        "Repositories (name: what it is):\n"
        "- zeta: Zeta things.\n"
        "- bare\n"
        "- alpha: Alpha things."
    )


# parse


NAMES = ["app", "lib"]


def test_parse_plain_json():
    assert route.parse('{"repo": "app", "reason": "billing"}', NAMES) == route.Answer("app", "billing")


def test_parse_fenced_json():
    assert route.parse('```json\n{"repo": "lib", "reason": "r"}\n```', NAMES) == route.Answer("lib", "r")


def test_parse_json_among_prose():
    text = 'Sure! Here you go: {"repo": "app", "reason": "x"} Hope it helps.'
    assert route.parse(text, NAMES) == route.Answer("app", "x")


def test_parse_json_followed_by_braces_in_prose():
    text = '{"repo":"app","reason":"x"} (see {docs})'
    assert route.parse(text, NAMES) == route.Answer("app", "x")


def test_parse_null_none_and_empty_mean_no_repo():
    assert route.parse('{"repo": null, "reason": "unclear"}', NAMES) == route.Answer(None, "unclear")
    assert route.parse('{"repo": "none"}', NAMES) == route.Answer(None, "")
    assert route.parse('{"repo": "null"}', NAMES) == route.Answer(None, "")
    assert route.parse('{"repo": "None"}', NAMES) == route.Answer(None, "")
    assert route.parse('{"repo": ""}', NAMES) == route.Answer(None, "")
    assert route.parse("{}", NAMES) == route.Answer(None, "")


@pytest.mark.parametrize(
    ("text", "error"),
    [
        ("I cannot tell.", "not a JSON object"),
        ('["app"]', "not a JSON object"),
        ('{"repo": "app"', "invalid json"),
        ("{not json}", "invalid json"),
        ('{"repo": "other", "reason": "x"}', "'other', not a candidate"),
        ('{"repo": 5}', "repo"),
        ('{"repo": "app", "reason": 5}', "reason"),
    ],
)
def test_parse_rejects_what_is_not_an_answer(text: str, error: str):
    with pytest.raises(LlmError, match=error):
        route.parse(text, NAMES)


# route


def test_route_asks_the_router_prompt_with_no_extra_dirs(memory_repo: Path, tmp_path: Path):
    app = make_repo(tmp_path, "app", "Billing.\n")
    fake = answering("app")
    assert route.route("pay the bill", [app], fake) == route.Answer("app", "because")
    (call,) = fake.calls
    assert call.add_dirs == ()
    assert call.system == router_prompt()
    assert call.message.startswith("Task:\npay the bill\n")


def test_router_prompt_is_filled():
    assert "@@" not in router_prompt()


def test_route_uses_the_haiku_low_runner_without_tools(memory_repo: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    seen: list[dict[str, object]] = []

    def factory(**kwargs: object) -> FakeLlm:
        seen.append(kwargs)
        return answering("app")

    monkeypatch.setattr("knowledge.route.ClaudeCli", factory)
    assert route.route("t", [make_repo(tmp_path, "app")]).repo == "app"
    assert seen == [{"model": "haiku", "effort": "low", "timeout": 60, "tools": ""}]


def test_route_with_an_unknown_repo_is_an_error(memory_repo: Path, tmp_path: Path):
    with pytest.raises(LlmError, match="not a candidate"):
        route.route("t", [make_repo(tmp_path, "app")], answering("elsewhere"))


# the command


def stdin_of(*repos: Path) -> str:
    return "".join(f"{r}\n" for r in repos)


def test_route_prints_one_json_line(run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path, tmp_path: Path):
    app = make_repo(tmp_path, "app", "Billing.\n")
    lib = make_repo(tmp_path, "lib", "Parsing.\n")
    fake = fake_llm(monkeypatch, answering("app", "it bills"))
    code, out = run("route", "pay", "the", "bill", stdin=stdin_of(app, lib))
    assert (code, out) == (0, '{"repo":"app","reason":"it bills"}\n')
    assert fake.calls[0].message == (
        "Task:\npay the bill\n\nRepositories (name: what it is):\n- app: Billing.\n- lib: Parsing."
    )


def test_route_prints_null_when_there_is_no_clear_repo(
    run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path, tmp_path: Path
):
    fake_llm(monkeypatch, answering(None, "unclear"))
    code, out = run("route", "t", stdin=stdin_of(make_repo(tmp_path, "app")))
    assert (code, out) == (0, '{"repo":null,"reason":"unclear"}\n')


def test_route_keeps_the_newlines_of_a_single_argument(
    run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path, tmp_path: Path
):
    fake = fake_llm(monkeypatch, answering("app"))
    assert run("route", "first\nsecond", stdin=f"  {make_repo(tmp_path, 'app')}  \n\n")[0] == 0
    assert fake.calls[0].message.startswith("Task:\nfirst\nsecond\n\n")


def test_route_after_dashes_takes_a_task_that_starts_with_a_dash(
    run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path, tmp_path: Path
):
    fake = fake_llm(monkeypatch, answering("app"))
    assert run("route", "--", "-x", stdin=stdin_of(make_repo(tmp_path, "app")))[0] == 0
    assert fake.calls[0].message.startswith("Task:\n-x\n")


def test_route_usage_errors_exit_2_without_a_call(
    run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path, tmp_path: Path
):
    fake = fake_llm(monkeypatch, FakeLlm())
    app = stdin_of(make_repo(tmp_path, "app"))
    assert run("route", stdin=app) == (2, "")  # no task
    assert run("route", " ", stdin=app) == (2, "")
    assert run("route", "t", stdin="") == (2, "")  # no candidates
    assert run("route", "t", stdin=" \n\n") == (2, "")
    assert run("route", "t", stdin=app, tty=True) == (2, "")  # a terminal is no candidates
    assert fake.calls == []


def test_route_without_a_memory_dir_is_a_usage_error(run: Runner, monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    fake = fake_llm(monkeypatch, FakeLlm())
    assert run("route", "t", stdin=stdin_of(make_repo(tmp_path, "app"))) == (2, "")
    assert fake.calls == []


def test_route_when_the_call_fails_exits_1(
    run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path, tmp_path: Path, caplog: pytest.LogCaptureFixture
):
    fake_llm(monkeypatch, FakeLlm(error=LlmError("claude timed out after 60s")))
    assert run("route", "t", stdin=stdin_of(make_repo(tmp_path, "app"))) == (1, "")
    assert "claude timed out after 60s" in caplog.text


def test_route_with_an_unusable_answer_exits_1(
    run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path, tmp_path: Path, caplog: pytest.LogCaptureFixture
):
    fake_llm(monkeypatch, FakeLlm(lambda message: "no idea"))
    assert run("route", "t", stdin=stdin_of(make_repo(tmp_path, "app"))) == (1, "")
    assert "no idea" in caplog.text


def test_route_with_a_non_candidate_answer_exits_1(
    run: Runner, monkeypatch: pytest.MonkeyPatch, memory_repo: Path, tmp_path: Path, caplog: pytest.LogCaptureFixture
):
    fake_llm(monkeypatch, answering("elsewhere"))
    assert run("route", "t", stdin=stdin_of(make_repo(tmp_path, "app"))) == (1, "")
    assert "not a candidate" in caplog.text
