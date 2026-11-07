import pytest

from fakes import HOST, FakeBus, FakeLocal, item, permission, question
from matrix_inbox import handlers, logic
from matrix_inbox.data import Obj, dumps, loads
from matrix_inbox.handlers import BusError, LocalError
from matrix_inbox.signed import Reply

ALLOW = '{"behavior":"allow"}'
DENY = '{"behavior":"deny"}'
EVENT_ID = "$owner1"
GONE = "No longer waiting: answered elsewhere or expired."
MISMATCH = "Refusing: the message does not match the waiting request."
UNREADABLE = "Failed: cannot read the inbox."
ANSWERED = "✅ Answered."
TOO_LONG = "Too long to approve here: reply n to deny, or answer it on the computer."


def reply_to(event: Obj, text: str, ref: str | None = None, body: str | None = None, to: str = HOST) -> Reply:
    ref_value = ref if ref is not None else event.get("reply")
    assert isinstance(ref_value, str)
    return Reply(to, ref_value, text, logic.render(event) if body is None else body, EVENT_ID)


def run(reply: Reply, items: list[Obj]) -> tuple[FakeLocal, FakeBus]:
    local = FakeLocal(items)
    bus = FakeBus()
    handlers.answer(reply, HOST, local, bus)
    return local, bus


def parsed(text: str) -> Obj:
    value = loads(text)
    assert isinstance(value, dict)
    return value


def answer_json(*pairs: tuple[str, str]) -> str:
    return dumps({"behavior": "allow", "answers": dict(pairs)})


# post


def test_post_permission() -> None:
    event = permission(reply="/r/claude-hook-x/reply")
    bus = FakeBus()
    handlers.post(event, HOST, bus)
    assert len(bus.published) == 1
    assert bus.published[0][0] == "matrix.send"
    assert parsed(bus.published[0][1]) == {"to": "yenn", "ref": "/r/claude-hook-x/reply", "body": logic.render(event)}


def test_post_question() -> None:
    event = question(reply="/r/q1")
    bus = FakeBus()
    handlers.post(event, "yenn", bus)
    assert len(bus.published) == 1
    assert bus.published[0][0] == "matrix.send"
    assert parsed(bus.published[0][1]) == {"to": "yenn", "ref": "/r/q1", "body": logic.render(event)}


def test_post_is_one_line_and_not_ascii_escaped() -> None:
    event = permission(command="é\nž")
    bus = FakeBus()
    handlers.post(event, HOST, bus)
    data = bus.published[0][1]
    assert "\n" not in data
    assert "é" in data
    assert "\\u00e9" not in data
    assert parsed(data)["body"] == logic.render(event)


@pytest.mark.parametrize(
    "event",
    [
        question(reply=None),
        question(reply=""),
        {"type": "sudo.password_request", "reply": "/r/x"},
        {"type": "claude.waiting", "reply": "/r/x"},
        {"type": "task.failed", "reply": "/r/x"},
    ],
    ids=["no-reply", "empty-reply", "sudo", "waiting", "task-failed"],
)
def test_post_not_offered(event: Obj) -> None:
    bus = FakeBus()
    handlers.post(event, HOST, bus)
    assert bus.published == []


def test_post_bus_error_propagates() -> None:
    with pytest.raises(BusError):
        handlers.post(permission(), HOST, FakeBus(error=BusError("down")))


# answer


def test_answer_allow_permission() -> None:
    event = permission()
    local, bus = run(reply_to(event, "y"), [item("A", event)])
    assert local.replies == [("A", ALLOW)]
    assert bus.notices() == [ANSWERED]
    assert parsed(bus.published[0][1])["reply_to"] == EVENT_ID


def test_answer_text_is_stripped() -> None:
    event = permission()
    local, _ = run(reply_to(event, "  y\n"), [item("A", event)])
    assert local.replies == [("A", ALLOW)]


def test_answer_deny_permission() -> None:
    event = permission()
    local, bus = run(reply_to(event, "n"), [item("A", event)])
    assert local.replies == [("A", DENY)]
    assert bus.notices() == [ANSWERED]


def test_answer_permission_other_text_is_refused() -> None:
    event = permission()
    local, bus = run(reply_to(event, "maybe"), [item("A", event)])
    assert local.replies == []
    assert bus.notices() == ["Reply y or n."]


def test_answer_notice_is_one_line_json_not_ascii_escaped() -> None:
    event = permission()
    local = FakeLocal([item("A", event)])
    local.reply_results["A"] = "żółć"
    bus = FakeBus()
    handlers.answer(reply_to(event, "y"), HOST, local, bus)
    data = bus.published[0][1]
    assert "żółć" in data
    assert "\n" not in data


def test_answer_question_number() -> None:
    event = question()
    local, bus = run(reply_to(event, "1"), [item("Q", event)])
    assert local.replies == [("Q", answer_json(("Which db?", "A")))]
    assert bus.notices() == [ANSWERED]


def test_answer_question_free_text() -> None:
    event = question()
    local, bus = run(reply_to(event, "my own"), [item("Q", event)])
    assert local.replies == [("Q", answer_json(("Which db?", "my own")))]
    assert bus.notices() == [ANSWERED]


def test_answer_question_out_of_range_is_refused() -> None:
    event = question()
    local, bus = run(reply_to(event, "9"), [item("Q", event)])
    assert local.replies == []
    assert bus.notices() == ["Question 1: there is no option 9."]


def test_answer_question_empty_is_refused() -> None:
    event = question()
    local, bus = run(reply_to(event, "  "), [item("Q", event)])
    assert local.replies == []
    assert bus.notices() == ["Empty answer."]


def test_answer_question_without_usable_tool_input_is_refused() -> None:
    event = question()
    event["tool_input"] = "oops"
    local, bus = run(reply_to(event, "1"), [item("Q", event)])
    assert local.replies == []
    assert bus.notices() == ["This question cannot be answered here."]


def test_answer_wrong_host_does_not_even_read_the_inbox() -> None:
    event = permission()
    local = FakeLocal([item("A", event)])
    local.items_error = LocalError("boom")
    bus = FakeBus()
    handlers.answer(reply_to(event, "y", to="geralt"), HOST, local, bus)
    assert local.replies == []
    assert bus.published == []


def test_answer_item_gone() -> None:
    event = permission()
    local, bus = run(reply_to(event, "y", ref="/r/gone"), [item("A", event)])
    assert local.replies == []
    assert bus.notices() == [GONE]
    assert parsed(bus.published[0][1])["reply_to"] == EVENT_ID


def test_answer_empty_inbox() -> None:
    event = permission()
    local, bus = run(reply_to(event, "y"), [])
    assert local.replies == []
    assert bus.notices() == [GONE]


def test_answer_claimant_event_not_offered() -> None:
    waiting: Obj = {"type": "claude.waiting", "reply": "/r/p"}
    local, bus = run(Reply(HOST, "/r/p", "y", "whatever", EVENT_ID), [item("A", waiting)])
    assert local.replies == []
    assert bus.notices() == [GONE]


@pytest.mark.parametrize(
    "first",
    [
        {"type": "sudo.password_request", "reply": "r"},
        {"type": "claude.waiting", "reply": "r"},
    ],
    ids=["sudo", "no-longer-waiting"],
)
def test_answer_non_offered_claimant_hides_a_later_offered_item(first: Obj) -> None:
    later = permission(reply="r")
    local, bus = run(reply_to(later, "y"), [item("A", first), item("B", later)])
    assert local.replies == []
    assert bus.notices() == [GONE]


def test_answer_body_of_a_later_item_with_same_ref_is_a_mismatch() -> None:
    first = permission(reply="r", title="first")
    second = permission(reply="r", title="second")
    assert logic.render(first) != logic.render(second)
    local, bus = run(reply_to(second, "y"), [item("A", first), item("B", second)])
    assert local.replies == []
    assert bus.notices() == [MISMATCH]


def test_answer_body_differs_by_one_char() -> None:
    event = permission()
    body = logic.render(event)
    local, bus = run(reply_to(event, "y", body=body[:-1] + ("!" if body[-1] != "!" else "?")), [item("A", event)])
    assert local.replies == []
    assert bus.notices() == [MISMATCH]


def test_answer_body_empty_is_a_mismatch() -> None:
    event = permission()
    local, bus = run(reply_to(event, "y", body=""), [item("A", event)])
    assert local.replies == []
    assert bus.notices() == [MISMATCH]


def test_answer_matches_despite_key_order_and_number_spelling() -> None:
    posted = loads(
        '{"type":"claude.permission_request","reply":"/r/p","project":"infra","tool_name":"Edit",'
        '"title":"t","tool_input":{"a":1.0,"b":2}}'
    )
    current = loads(
        '{"tool_input":{"b":2,"a":1.00},"title":"t","tool_name":"Edit","project":"infra",'
        '"reply":"/r/p","type":"claude.permission_request"}'
    )
    assert isinstance(posted, dict)
    assert isinstance(current, dict)
    local, bus = run(reply_to(posted, "y"), [item("A", current)])
    assert local.replies == [("A", ALLOW)]
    assert bus.notices() == [ANSWERED]


def test_answer_truncated_permission_allow_is_refused() -> None:
    event = permission(command="x" * 2500)
    local, bus = run(reply_to(event, "y"), [item("A", event)])
    assert local.replies == []
    assert bus.notices() == [TOO_LONG]


def test_answer_truncated_permission_deny_is_sent() -> None:
    event = permission(command="x" * 2500)
    local, bus = run(reply_to(event, "n"), [item("A", event)])
    assert local.replies == [("A", DENY)]
    assert bus.notices() == [ANSWERED]


def test_answer_local_reply_failure() -> None:
    event = permission()
    local = FakeLocal([item("A", event)])
    local.reply_results["A"] = "expired: nothing is waiting for the answer"
    bus = FakeBus()
    handlers.answer(reply_to(event, "y"), HOST, local, bus)
    assert local.replies == [("A", ALLOW)]
    assert bus.notices() == ["Failed: expired: nothing is waiting for the answer"]


def test_answer_local_reply_failure_is_sanitized_single_line_and_capped() -> None:
    event = permission()
    local = FakeLocal([item("A", event)])
    local.reply_results["A"] = "l1\nl2\x1b" + "x" * 400
    bus = FakeBus()
    handlers.answer(reply_to(event, "y"), HOST, local, bus)
    (notice,) = bus.notices()
    assert notice == "Failed: " + ("l1 l2�" + "x" * 400)[:300]


def test_answer_inbox_unreadable() -> None:
    event = permission()
    local = FakeLocal([item("A", event)])
    local.items_error = LocalError("agent-inbox list: boom")
    bus = FakeBus()
    handlers.answer(reply_to(event, "y"), HOST, local, bus)
    assert local.replies == []
    assert bus.notices() == [UNREADABLE]


@pytest.mark.parametrize("text", ["y", "n"])
def test_answer_bus_error_on_notice_propagates(text: str) -> None:
    event = permission()
    local = FakeLocal([item("A", event)])
    with pytest.raises(BusError):
        handlers.answer(reply_to(event, text), HOST, local, FakeBus(error=BusError("down")))


def test_answer_bus_error_on_gone_notice_propagates() -> None:
    event = permission()
    with pytest.raises(BusError):
        handlers.answer(reply_to(event, "y", ref="/r/gone"), HOST, FakeLocal([]), FakeBus(error=BusError("down")))
