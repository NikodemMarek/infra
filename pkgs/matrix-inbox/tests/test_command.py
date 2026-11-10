import pytest

from fakes import HOST, FakeBus, FakeSeen, FakeTasks
from matrix_inbox import handlers
from matrix_inbox.data import Json, Obj
from matrix_inbox.handlers import BusError
from matrix_inbox.signed import Command

TS = 1700000000000
CMD_ID = "$cmd1"
NOW = TS / 1000 + 10
NO_RESULT = "Failed: the task starter gave no result."
STARTED: Obj = {"type": "task.started", "project": "infra", "shpool": "s-1110-120000"}


def cmd(to: str = HOST, text: str = "fix it", event_id: str = CMD_ID, ts: int = TS) -> Command:
    return Command(to, text, event_id, ts)


def run(
    c: Command,
    outcome: Obj | None = None,
    now: float = NOW,
    seen: FakeSeen | None = None,
    bus: FakeBus | None = None,
) -> tuple[FakeSeen, FakeTasks, FakeBus]:
    seen = seen if seen is not None else FakeSeen()
    tasks = FakeTasks(outcome)
    bus = bus if bus is not None else FakeBus()
    handlers.command(c, HOST, now, seen, tasks, bus)
    return seen, tasks, bus


def test_max_age() -> None:
    assert handlers.MAX_AGE == 300


def test_started() -> None:
    seen, tasks, bus = run(cmd(), STARTED)
    assert seen.calls == [CMD_ID]
    assert tasks.prompts == ["fix it"]
    assert bus.notices() == ["Started s-1110-120000 in infra."]
    assert bus.payloads()[0]["reply_to"] == CMD_ID


def test_text_is_passed_verbatim() -> None:
    _, tasks, _ = run(cmd(text="fix\nthe ż test"), STARTED)
    assert tasks.prompts == ["fix\nthe ż test"]


def test_started_missing_fields_are_empty() -> None:
    _, _, bus = run(cmd(), {"type": "task.started"})
    assert bus.notices() == ["Started  in ."]


def test_failed() -> None:
    _, _, bus = run(cmd(), {"type": "task.failed", "reason": "no repo chosen (no clear match)"})
    assert bus.notices() == ["Failed: no repo chosen (no clear match)"]


def test_failed_reason_single_line_and_truncated() -> None:
    reason = ("ab\n" * 134)[:400]
    assert len(reason) == 400 and "\n" in reason
    _, _, bus = run(cmd(), {"type": "task.failed", "reason": reason})
    (body,) = bus.notices()
    assert body.startswith("Failed: ")
    assert "\n" not in body
    assert len(body) == len("Failed: ") + 300


def test_failed_reason_of_300_is_kept() -> None:
    _, _, bus = run(cmd(), {"type": "task.failed", "reason": "x" * 300})
    assert bus.notices() == ["Failed: " + "x" * 300]


def test_failed_reason_control_chars_are_sanitized() -> None:
    _, _, bus = run(cmd(), {"type": "task.failed", "reason": "a\x1b[31mb\tc"})
    (body,) = bus.notices()
    assert "\x1b" not in body
    assert "\t" not in body
    assert body.startswith("Failed: a")


@pytest.mark.parametrize("reason", [None, 5, ["x"], {"a": "b"}])
def test_failed_reason_missing_or_not_a_string(reason: Json) -> None:
    outcome: Obj = {"type": "task.failed"} if reason is None else {"type": "task.failed", "reason": reason}
    _, _, bus = run(cmd(), outcome)
    assert bus.notices() == ["Failed: unknown"]


def test_no_result() -> None:
    _, tasks, bus = run(cmd(), None)
    assert tasks.prompts == ["fix it"]
    assert bus.notices() == [NO_RESULT]


@pytest.mark.parametrize("outcome", [{}, {"type": "other"}, {"type": 1}, {"project": "infra", "shpool": "s-1"}])
def test_unknown_outcome(outcome: Obj) -> None:
    _, _, bus = run(cmd(), outcome)
    assert bus.notices() == [NO_RESULT]


def test_wrong_host_does_nothing() -> None:
    seen, tasks, bus = run(cmd(to="shani"), STARTED)
    assert seen.calls == []
    assert tasks.prompts == []
    assert bus.published == []


def test_wrong_host_and_stale_does_nothing() -> None:
    seen, tasks, bus = run(cmd(to="shani"), STARTED, now=TS / 1000 + 1000)
    assert (seen.calls, tasks.prompts, bus.published) == ([], [], [])


@pytest.mark.parametrize("delta", [301, -301, 3600, -3600])
def test_stale_does_nothing(delta: float) -> None:
    seen, tasks, bus = run(cmd(), STARTED, now=TS / 1000 + delta)
    assert seen.calls == []
    assert tasks.prompts == []
    assert bus.published == []


@pytest.mark.parametrize("delta", [300, -300, 0, 299, -299])
def test_within_age_is_handled(delta: float) -> None:
    seen, tasks, bus = run(cmd(), STARTED, now=TS / 1000 + delta)
    assert seen.calls == [CMD_ID]
    assert tasks.prompts == ["fix it"]
    assert len(bus.published) == 1


def test_stale_does_not_consume_the_claim() -> None:
    seen = FakeSeen()
    run(cmd(), STARTED, now=TS / 1000 + 1000, seen=seen)
    assert seen.claimed == set()
    _, tasks, bus = run(cmd(), STARTED, seen=seen)
    assert tasks.prompts == ["fix it"]
    assert len(bus.published) == 1


def test_wrong_host_does_not_consume_the_claim() -> None:
    seen = FakeSeen()
    run(cmd(to="shani"), STARTED, seen=seen)
    assert seen.claimed == set()


def test_replayed_does_nothing() -> None:
    seen, tasks, bus = run(cmd(), STARTED, seen=FakeSeen({CMD_ID}))
    assert seen.calls == [CMD_ID]
    assert tasks.prompts == []
    assert bus.published == []


def test_same_command_twice_starts_once() -> None:
    seen = FakeSeen()
    tasks = FakeTasks(STARTED)
    bus = FakeBus()
    handlers.command(cmd(), HOST, NOW, seen, tasks, bus)
    handlers.command(cmd(), HOST, NOW, seen, tasks, bus)
    assert tasks.prompts == ["fix it"]
    assert bus.notices() == ["Started s-1110-120000 in infra."]


def test_different_events_both_start() -> None:
    seen = FakeSeen()
    tasks = FakeTasks(STARTED)
    bus = FakeBus()
    handlers.command(cmd(event_id="$a"), HOST, NOW, seen, tasks, bus)
    handlers.command(cmd(event_id="$b"), HOST, NOW, seen, tasks, bus)
    assert len(tasks.prompts) == 2
    assert [p["reply_to"] for p in bus.payloads()] == ["$a", "$b"]


def test_bus_error_propagates_after_start() -> None:
    seen = FakeSeen()
    tasks = FakeTasks(STARTED)
    with pytest.raises(BusError):
        handlers.command(cmd(), HOST, NOW, seen, tasks, FakeBus(BusError("down")))
    assert tasks.prompts == ["fix it"]
    assert seen.claimed == {CMD_ID}


def test_seen_oserror_propagates_without_starting() -> None:
    tasks = FakeTasks(STARTED)
    bus = FakeBus()
    with pytest.raises(OSError):
        handlers.command(cmd(), HOST, NOW, FakeSeen(error=OSError("read-only")), tasks, bus)
    assert tasks.prompts == []
    assert bus.published == []
