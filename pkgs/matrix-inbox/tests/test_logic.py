import pytest

from fakes import item, permission, question
from matrix_inbox import logic
from matrix_inbox.data import Obj, ParseError, dumps, loads
from matrix_inbox.logic import Refusal

TOO_LONG = "Too long to approve here: reply n to deny, or answer it on the computer."
ALLOW = '{"behavior":"allow"}'
DENY = '{"behavior":"deny"}'


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("a\x1bb", "a�b"),
        ("a‮b", "a�b"),
        ("a​b", "a�b"),
        ("a\x7fb", "a�b"),
        ("a\rb", "a�b"),
        ("a b", "a�b"),
        ("a b", "a�b"),
        ("a\nb", "a\nb"),
        ("a\tb", "a\tb"),
        ("", ""),
        ("plain é text", "plain é text"),
    ],
)
def test_sanitize(text: str, expected: str) -> None:
    assert logic.sanitize(text) == expected


def test_sanitize_single_line_line_separators_are_replaced() -> None:
    assert logic.sanitize("a b c", single_line=True) == "a�b�c"


def test_sanitize_single_line() -> None:
    assert logic.sanitize("a\nb\tc", single_line=True) == "a b c"
    assert logic.sanitize("a\x1b\rb\n", single_line=True) == "a��b "
    assert logic.sanitize("", single_line=True) == ""


def test_offered() -> None:
    assert logic.offered(question(reply="/r/claude-hook-x/reply"))
    assert logic.offered(permission())
    assert not logic.offered(question(reply=None))
    assert not logic.offered(question(reply=""))
    assert not logic.offered({"type": "sudo.password_request", "reply": "/r/x"})
    assert not logic.offered({"type": "claude.waiting", "reply": "/r/x"})
    assert not logic.offered({"type": "task.failed", "reply": "/r/x"})
    assert not logic.offered({"type": "claude.question", "reply": 5})
    assert not logic.offered({"reply": "/r/x"})


def test_claimant() -> None:
    a = item("A", question(reply="r"))
    b = item("B", question(reply="r"))
    c = item("C", question(reply="r2"))
    assert logic.claimant([a, b], "r") == "A"
    assert logic.claimant([a, c], "r2") == "C"
    assert logic.claimant([b, a], "r") == "B"
    assert logic.claimant([a, c], "nope") is None
    assert logic.claimant([], "r") is None


def test_detail_bash_command() -> None:
    assert logic.detail({"tool_name": "Bash", "tool_input": {"command": "ls -la"}}) == ("ls -la", False)


def test_detail_truncates_long_command() -> None:
    text, truncated = logic.detail({"tool_name": "Bash", "tool_input": {"command": "x" * 2500}})
    assert truncated
    assert text == "x" * 2000 + "\n[500 more chars]"


def test_detail_boundary() -> None:
    assert logic.detail({"tool_name": "Bash", "tool_input": {"command": "x" * 2000}}) == ("x" * 2000, False)
    assert logic.detail({"tool_name": "Bash", "tool_input": {"command": "x" * 2001}}) == (
        "x" * 2000 + "\n[1 more chars]",
        True,
    )


def test_detail_other_tool_dumps_json() -> None:
    assert logic.detail({"tool_name": "Edit", "tool_input": {"file_path": "/x"}}) == ('{\n  "file_path": "/x"\n}', False)
    assert logic.detail({"tool_name": "Edit", "tool_input": {"b": 1, "a": "é"}}) == (
        '{\n  "a": "é",\n  "b": 1\n}',
        False,
    )


def test_detail_without_tool_input() -> None:
    assert logic.detail({"tool_name": "Bash"}) == ("{}", False)
    assert logic.detail({"tool_name": "Bash", "tool_input": "oops"}) == ("{}", False)


def test_detail_bash_without_string_command_dumps_json() -> None:
    assert logic.detail({"tool_name": "Bash", "tool_input": {"command": 5}}) == ('{\n  "command": 5\n}', False)


def test_detail_sanitizes() -> None:
    assert logic.detail({"tool_name": "Bash", "tool_input": {"command": "a\x1bb"}}) == ("a�b", False)


def test_render_permission() -> None:
    assert logic.render(permission()) == "infra · s-1: Bash\nList files\n\nls -la\n\nReply y to allow or n to deny."


def test_render_ignores_source_key_order_and_number_spelling() -> None:
    first = loads('{"type":"claude.permission_request","reply":"/r","tool_name":"Edit","tool_input":{"a":1.0,"b":2}}')
    second = loads('{"tool_input":{"b":2,"a":1.00},"tool_name":"Edit","reply":"/r","type":"claude.permission_request"}')
    assert isinstance(first, dict)
    assert isinstance(second, dict)
    assert logic.render(first) == logic.render(second)


def test_render_permission_truncated_hint() -> None:
    body = logic.render(permission(command="x" * 2500))
    assert body == "infra · s-1: Bash\nList files\n\n" + "x" * 2000 + "\n[500 more chars]\n\n" + TOO_LONG


def test_render_permission_with_huge_title_keeps_detail_and_hint() -> None:
    assert logic.TITLE_MAX == 200
    body = logic.render(permission(title="t" * 15000))
    assert body == "infra · s-1: Bash\n" + "t" * 200 + "\n\nls -la\n\nReply y to allow or n to deny."
    assert not body.endswith("[truncated]")


def test_render_permission_title_capped_single_line() -> None:
    body = logic.render(permission(title="a b\n" + "t" * 15000))
    title = body.split("\n")[1]
    assert title.startswith("a�b t")
    assert len(title) == 200
    assert body.endswith("\n\nls -la\n\nReply y to allow or n to deny.")


def test_render_permission_defaults() -> None:
    event: Obj = {"type": "claude.permission_request", "reply": "/r"}
    body = logic.render(event)
    assert body == "?: ?\nclaude.permission_request\n\n{}\n\nReply y to allow or n to deny."


def test_render_session_falls_back_to_shpool() -> None:
    body = logic.render(permission(session="", shpool="sh-2"))
    assert body.startswith("infra · sh-2: Bash\n")
    no_session = permission()
    del no_session["session"]
    assert logic.render(no_session).startswith("infra: Bash\n")


def test_render_title_sanitized_single_line() -> None:
    body = logic.render(permission(title="a\nb"))
    assert body.splitlines()[1] == "a b"


def test_render_header_fields_sanitized() -> None:
    body = logic.render(permission(project="in\nfra", session="s\x1b1", tool_name="Ba\tsh"))
    assert body.splitlines()[0] == "in fra · s�1: Ba sh"


def test_render_permission_detail_keeps_newlines() -> None:
    body = logic.render(permission(command="a\nb"))
    assert "\n\na\nb\n\n" in body


def test_render_question() -> None:
    event: Obj = {
        "type": "claude.question",
        "project": "infra",
        "tool_input": {
            "questions": [{"question": "Which db?", "options": [{"label": "A"}, {"label": "B", "description": "fast"}]}]
        },
        "reply": "/r",
    }
    assert logic.render(event) == (
        "infra asks:\nWhich db?\n   1) A\n   2) B — fast\n\nReply with the option number or your own answer."
    )


def test_render_question_header_session_and_multiselect() -> None:
    event: Obj = {
        "type": "claude.question",
        "project": "infra",
        "session": "s-9",
        "tool_input": {
            "questions": [
                {"header": "DB", "question": "Which?", "multiSelect": True, "options": [{"label": "A"}, {"label": "B"}]}
            ]
        },
        "reply": "/r",
    }
    assert logic.render(event) == (
        "infra · s-9 asks:\nDB: Which?\n   1) A\n   2) B\n   (several: e.g. 1,3)\n\n"
        "Reply with the option number or your own answer."
    )


def test_render_multiple_questions() -> None:
    event: Obj = {
        "type": "claude.question",
        "project": "infra",
        "tool_input": {
            "questions": [
                {"header": "DB", "question": "Which db?", "options": [{"label": "A"}]},
                {
                    "question": "Which colors?",
                    "multiSelect": True,
                    "options": [{"label": "red", "description": "warm"}, {"label": "blue"}],
                },
                {"question": "Anything else?"},
            ]
        },
        "reply": "/r",
    }
    assert logic.render(event) == (
        "infra asks:\n"
        "1. DB: Which db?\n"
        "   1) A\n"
        "2. Which colors?\n"
        "   1) red — warm\n"
        "   2) blue\n"
        "   (several: e.g. 1,3)\n"
        "3. Anything else?\n"
        "\n"
        "Reply with one line per question: the option number(s) or your own answer."
    )


def test_render_question_text_keeps_newlines_and_sanitizes() -> None:
    event = question()
    event["tool_input"] = {"questions": [{"question": "l1\nl2\x1b"}]}
    assert "l1\nl2�" in logic.render(event)


def test_render_body_capped() -> None:
    event = question()
    event["tool_input"] = {"questions": [{"question": "y" * 20000}]}
    body = logic.render(event)
    assert body.endswith("\n[truncated]")
    assert len(body) == 16000 + len("\n[truncated]")
    assert body.startswith("infra asks:\nyyy")


@pytest.mark.parametrize("text", ["y", "Y", "yes", " YES ", "allow", "Allow\n"])
def test_parse_permission_allow(text: str) -> None:
    assert logic.parse_permission(text, False) == ALLOW


@pytest.mark.parametrize("text", ["n", "No", " no ", "deny", "DENY"])
@pytest.mark.parametrize("truncated", [False, True])
def test_parse_permission_deny(text: str, truncated: bool) -> None:
    assert logic.parse_permission(text, truncated) == DENY


@pytest.mark.parametrize("text", ["y", "yes", "allow"])
def test_parse_permission_allow_refused_when_truncated(text: str) -> None:
    assert logic.parse_permission(text, True) == Refusal(TOO_LONG)


@pytest.mark.parametrize("text", ["maybe", "", "y n", "yes please"])
@pytest.mark.parametrize("truncated", [False, True])
def test_parse_permission_other(text: str, truncated: bool) -> None:
    assert logic.parse_permission(text, truncated) == Refusal("Reply y or n.")


def answers(*pairs: tuple[str, str]) -> str:
    given: Obj = {question: answer for question, answer in pairs}
    return dumps({"behavior": "allow", "answers": given})


def one_question(multi: bool = False, options: bool = True) -> Obj:
    q: Obj = {"question": "Which db?"}
    if options:
        q["options"] = [{"label": "A"}, {"label": "B"}, {"label": "C"}]
    if multi:
        q["multiSelect"] = True
    return {"questions": [q]}


def test_parse_question_number() -> None:
    assert logic.parse_question(one_question(), "2") == '{"behavior":"allow","answers":{"Which db?":"B"}}'
    assert logic.parse_question(one_question(), " 3 \n") == answers(("Which db?", "C"))


def test_parse_question_free_text() -> None:
    assert logic.parse_question(one_question(), "my own") == answers(("Which db?", "my own"))
    assert logic.parse_question(one_question(), "1 2") == answers(("Which db?", "1 2"))
    assert logic.parse_question(one_question(), "l1\nl2") == answers(("Which db?", "l1\nl2"))


def test_parse_question_free_text_is_one_line_json() -> None:
    result = logic.parse_question(one_question(), 'l1\nl2 "é"')
    assert isinstance(result, str)
    assert "\n" not in result
    assert "é" in result


def test_parse_question_empty() -> None:
    assert logic.parse_question(one_question(), "  ") == Refusal("Empty answer.")
    assert logic.parse_question(one_question(), "") == Refusal("Empty answer.")


def test_parse_question_multiselect() -> None:
    assert logic.parse_question(one_question(multi=True), "1, 3") == answers(("Which db?", "A, C"))
    assert logic.parse_question(one_question(multi=True), "3,1") == answers(("Which db?", "C, A"))
    assert logic.parse_question(one_question(multi=True), "1,1") == answers(("Which db?", "A"))
    assert logic.parse_question(one_question(multi=True), "2") == answers(("Which db?", "B"))


def test_parse_question_several_numbers_need_multiselect() -> None:
    assert logic.parse_question(one_question(), "1,2") == Refusal("Question 1: pick one option.")


def test_parse_question_multiselect_only_when_literally_true() -> None:
    tool_input: Obj = {
        "questions": [{"question": "Q", "multiSelect": "true", "options": [{"label": "A"}, {"label": "B"}]}]
    }
    assert logic.parse_question(tool_input, "1,2") == Refusal("Question 1: pick one option.")


def test_parse_question_out_of_range() -> None:
    assert logic.parse_question(one_question(), "4") == Refusal("Question 1: there is no option 4.")
    assert logic.parse_question(one_question(), "0") == Refusal("Question 1: there is no option 0.")
    assert logic.parse_question(one_question(multi=True), "1,7") == Refusal("Question 1: there is no option 7.")


@pytest.mark.parametrize("text", ["5" * 5000, "1000", "1,1000"], ids=["5000-digits", "1000", "1,1000"])
def test_parse_question_number_with_more_than_three_digits_is_refused(text: str) -> None:
    result = logic.parse_question(one_question(multi=True), text)
    assert isinstance(result, Refusal)
    assert result.reason.startswith("Question 1: there is no option ")


def test_parse_question_numbers_are_text_without_options() -> None:
    assert logic.parse_question(one_question(options=False), "1") == answers(("Which db?", "1"))
    tool_input: Obj = {"questions": [{"question": "Q", "options": [{"nolabel": 1}]}]}
    assert logic.parse_question(tool_input, "1") == answers(("Q", "1"))


def test_parse_question_two_questions() -> None:
    tool_input: Obj = {
        "questions": [
            {"question": "Which db?", "options": [{"label": "A"}, {"label": "B"}]},
            {"question": "Why?", "options": [{"label": "x"}]},
        ]
    }
    assert logic.parse_question(tool_input, "1\n\nfoo") == answers(("Which db?", "A"), ("Why?", "foo"))
    assert logic.parse_question(tool_input, "  2  \n 1 ") == answers(("Which db?", "B"), ("Why?", "x"))


def test_parse_question_two_questions_wrong_line_count() -> None:
    tool_input: Obj = {"questions": [{"question": "Q1"}, {"question": "Q2"}]}
    assert logic.parse_question(tool_input, "1") == Refusal("Reply with 2 lines, one per question.")
    assert logic.parse_question(tool_input, "a\nb\nc") == Refusal("Reply with 2 lines, one per question.")
    assert logic.parse_question(tool_input, "") == Refusal("Reply with 2 lines, one per question.")


def test_parse_question_two_questions_refusal_names_question() -> None:
    tool_input: Obj = {
        "questions": [
            {"question": "Q1", "options": [{"label": "A"}]},
            {"question": "Q2", "options": [{"label": "A"}, {"label": "B"}]},
        ]
    }
    assert logic.parse_question(tool_input, "1\n3") == Refusal("Question 2: there is no option 3.")
    assert logic.parse_question(tool_input, "1\n1,2") == Refusal("Question 2: pick one option.")


INVALID_TOOL_INPUTS: list[Obj] = [
    {},
    {"questions": []},
    {"questions": "x"},
    {"questions": ["x"]},
    {"questions": [{"question": ""}]},
    {"questions": [{"question": 5}]},
    {"questions": [{"options": []}]},
    {"questions": [{"question": "ok"}, {"question": ""}]},
]


@pytest.mark.parametrize("tool_input", INVALID_TOOL_INPUTS)
def test_parse_question_invalid(tool_input: Obj) -> None:
    assert logic.parse_question(tool_input, "1") == Refusal("This question cannot be answered here.")


def test_loads_invalid_utf8_raises_parse_error() -> None:
    with pytest.raises(ParseError):
        loads(b"\xff\xfe{")
