import re
from collections.abc import Mapping, Sequence

import pytest

from secret_broker.broker import GRANT_SECONDS, OUTPUT_CAP, Broker
from secret_broker.model import (
    Action,
    ActionInfo,
    Answer,
    Config,
    Denied,
    Failed,
    Pending,
    Ran,
    RunReq,
    RunResult,
)

CAP_MIB5 = 5 * 1024 * 1024


class Clock:
    def __init__(self) -> None:
        self.now = 1000.0

    def __call__(self) -> float:
        return self.now


class Env:
    """Fake clock, ask and run around a Broker."""

    def __init__(
        self,
        actions: dict[str, Action] | None = None,
        lock_after: int = 1440,
    ) -> None:
        self.clock = Clock()
        self.asked: list[Pending] = []
        self.answers: list[Answer] = []
        self.on_ask: list[tuple[Mapping[str, str], Mapping[str, str]]] = []
        self.runs: list[tuple[list[str], dict[str, str], int]] = []
        self.result = RunResult(0, b"out", b"err")
        if actions is None:
            actions = {"tok": make_action("tok", secrets={"TOK": "it"})}
        self.config = Config("u", lock_after, None, frozenset(), None, actions)
        self.broker = Broker(self.config, self.clock, self.ask, self.run)

    def ask(self, p: Pending) -> Answer:
        self.asked.append(p)
        if self.on_ask:
            values, errors = self.on_ask.pop(0)
            self.broker.unlock(values, errors)
        return self.answers.pop(0) if self.answers else "once"

    def run(self, argv: Sequence[str], env: Mapping[str, str], timeout: int) -> RunResult:
        self.runs.append((list(argv), dict(env), timeout))
        return self.result

    def unlock_on_next_ask(self, values: Mapping[str, str], errors: Mapping[str, str] | None = None) -> None:
        self.on_ask.append((values, errors or {}))


def make_action(
    name: str,
    args: tuple[str, ...] = (),
    secrets: dict[str, str] | None = None,
    timeout: int = 30,
    command: tuple[str, ...] = ("/nix/store/x/bin/tool", "--base"),
) -> Action:
    return Action(
        name,
        f"desc {name}",
        command,
        tuple(re.compile(a) for a in args),
        secrets or {},
        timeout,
    )


def req(action: str = "tok", *args: str) -> RunReq:
    return RunReq(action, tuple(args), "/cwd", "sess")


def test_constants() -> None:
    assert GRANT_SECONDS == 900
    assert OUTPUT_CAP == 4 * 1024 * 1024


def test_starts_locked() -> None:
    assert Env().broker.locked


def test_referenced_items() -> None:
    e = Env(
        {
            "a": make_action("a", secrets={"X": "zz", "Y": "aa"}),
            "b": make_action("b", secrets={"X": "zz", "Z": "mm"}),
            "c": make_action("c"),
        }
    )
    assert e.broker.referenced_items() == ["aa", "mm", "zz"]


def test_referenced_items_empty() -> None:
    assert Env({}).broker.referenced_items() == []


def test_locked_request_asks_locked_then_runs_and_masks() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.result = RunResult(0, b"x s3cret-val", b"e s3cret-val")
    r = e.broker.handle_run(req("tok"))
    assert len(e.asked) == 1
    assert e.asked[0].locked is True
    assert e.asked[0].action.name == "tok"
    assert e.asked[0].cwd == "/cwd"
    assert e.asked[0].session == "sess"
    assert e.runs == [(["/nix/store/x/bin/tool", "--base"], {"TOK": "s3cret-val"}, 30)]
    assert r == Ran(0, b"x [masked]", b"e [masked]", False)
    assert not e.broker.locked


def test_run_appends_args_and_passes_exit() -> None:
    e = Env({"a": make_action("a", args=("^[a-z]+$", r"\d+"))})
    e.result = RunResult(7, b"", b"")
    e.unlock_on_next_ask({})
    r = e.broker.handle_run(req("a", "abc", "12"))
    assert e.runs[0][0] == ["/nix/store/x/bin/tool", "--base", "abc", "12"]
    assert e.asked[0].args == ("abc", "12")
    assert r == Ran(7, b"", b"", False)


def test_once_without_unlock_is_vault_locked() -> None:
    e = Env()
    assert e.broker.handle_run(req()) == Denied("vault locked")
    assert e.runs == []
    assert len(e.asked) == 1


def test_unknown_action_no_ask() -> None:
    e = Env()
    assert e.broker.handle_run(req("nope")) == Denied("unknown action")
    assert e.asked == []
    assert e.runs == []


def test_wrong_arg_count_and_regex_miss_no_ask() -> None:
    e = Env({"a": make_action("a", args=("^[a-z]+$",))})
    assert e.broker.handle_run(req("a")) == Denied("arguments do not match")
    assert e.broker.handle_run(req("a", "abc", "def")) == Denied("arguments do not match")
    assert e.broker.handle_run(req("a", "ABC")) == Denied("arguments do not match")
    assert e.broker.handle_run(req("a", "abc\n")) == Denied("arguments do not match")
    assert e.broker.handle_run(req("a", "xabc!")) == Denied("arguments do not match")
    assert e.asked == []
    assert e.runs == []


def test_args_must_fullmatch_not_search() -> None:
    e = Env({"a": make_action("a", args=("[a-z]+",))})
    assert e.broker.handle_run(req("a", "abc1")) == Denied("arguments do not match")
    assert e.asked == []


def test_no_args_action_rejects_extra_args() -> None:
    e = Env()
    assert e.broker.handle_run(req("tok", "extra")) == Denied("arguments do not match")


def test_deny_and_timeout() -> None:
    e = Env()
    e.answers = ["deny", "timeout"]
    assert e.broker.handle_run(req()) == Denied("denied on the console")
    assert e.broker.handle_run(req()) == Denied("approval timed out")
    assert e.runs == []


def test_deny_after_unlock_in_ask_still_denied() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.answers = ["deny"]
    assert e.broker.handle_run(req()) == Denied("denied on the console")
    assert e.runs == []


def test_first_use_flag() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.broker.handle_run(req())
    e.broker.handle_run(req())
    assert [p.first_use for p in e.asked] == [True, False]
    assert [p.locked for p in e.asked] == [True, False]


def test_first_use_is_per_action() -> None:
    e = Env({"a": make_action("a"), "b": make_action("b")})
    e.unlock_on_next_ask({})
    e.broker.handle_run(req("a"))
    e.broker.handle_run(req("b"))
    e.broker.handle_run(req("a"))
    assert [(p.action.name, p.first_use) for p in e.asked] == [("a", True), ("b", True), ("a", False)]


def test_first_use_stays_true_after_denial() -> None:
    e = Env()
    e.answers = ["deny", "once"]
    e.broker.handle_run(req())
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.broker.handle_run(req())
    assert [p.first_use for p in e.asked] == [True, True]


def test_grant_skips_ask_until_exactly_900s() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.answers = ["once", "grant", "once"]
    e.broker.handle_run(req())  # first use, once
    e.broker.handle_run(req())  # not first use; unlocked, no grant -> ask: grant
    assert len(e.asked) == 2
    t0 = e.clock.now
    e.clock.now = t0 + 899
    assert isinstance(e.broker.handle_run(req()), Ran)
    assert len(e.asked) == 2
    assert len(e.runs) == 3
    e.clock.now = t0 + GRANT_SECONDS
    assert isinstance(e.broker.handle_run(req()), Ran)
    assert len(e.asked) == 3
    assert e.asked[2].locked is False


def test_grant_does_not_skip_first_use_ask() -> None:
    e = Env({"a": make_action("a", secrets={"A": "it"}), "b": make_action("b", secrets={"B": "it"})})
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.answers = ["grant", "once"]
    e.broker.handle_run(req("a"))
    e.broker.handle_run(req("b"))
    assert [(p.action.name, p.first_use) for p in e.asked] == [("a", True), ("b", True)]


def test_lock_clears_grant_and_asks_locked() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.answers = ["grant"]
    e.broker.handle_run(req())
    assert [a.state for a in e.broker.list_actions()] == ["granted"]
    e.broker.lock()
    assert e.broker.locked
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.broker.handle_run(req())
    assert len(e.asked) == 2
    assert e.asked[1].locked is True
    assert e.asked[1].first_use is False  # lock does not reset first use
    e.broker.handle_run(req())  # grant gone after lock: asks again
    assert len(e.asked) == 3
    assert e.asked[2].locked is False


def test_lock_clears_values() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.broker.handle_run(req())
    e.broker.lock()
    assert e.broker.handle_run(req()) == Denied("vault locked")


def test_item_error_after_unlock_no_ask() -> None:
    e = Env()
    e.broker.unlock({}, {"it": "item missing: it"})
    assert e.broker.handle_run(req()) == Denied("item missing: it")
    assert e.asked == []
    assert e.runs == []


def test_item_error_first_in_items_order() -> None:
    e = Env({"a": make_action("a", secrets={"Z": "zz", "A": "aa"})})
    e.broker.unlock({}, {"zz": "item ambiguous: zz", "aa": "item has no value: aa"})
    assert e.broker.handle_run(req("a")) == Denied("item has no value: aa")


def test_item_error_appearing_during_ask() -> None:
    e = Env()
    e.unlock_on_next_ask({}, {"it": "item missing: it"})
    assert e.broker.handle_run(req()) == Denied("item missing: it")
    assert len(e.asked) == 1
    assert e.runs == []


def test_item_error_of_other_action_does_not_block() -> None:
    e = Env({"a": make_action("a", secrets={"A": "ia"}), "b": make_action("b", secrets={"B": "ib"})})
    e.broker.unlock({"ia": "valueA1"}, {"ib": "item missing: ib"})
    assert isinstance(e.broker.handle_run(req("a")), Ran)
    assert e.runs[0][1] == {"A": "valueA1"}


def test_action_without_secrets_runs_with_empty_env() -> None:
    e = Env({"a": make_action("a")})
    e.unlock_on_next_ask({})
    assert isinstance(e.broker.handle_run(req("a")), Ran)
    assert e.runs[0][1] == {}


def test_idle_lock_exactly_at_threshold() -> None:
    e = Env(lock_after=10)
    e.broker.unlock({"it": "s3cret-val"}, {})
    t0 = e.clock.now
    e.clock.now = t0 + 599.9
    e.broker.tick()
    assert not e.broker.locked
    e.clock.now = t0 + 600
    e.broker.tick()
    assert e.broker.locked


def test_every_public_method_ticks() -> None:
    for call in (
        lambda b: b.locked,
        lambda b: b.list_actions(),
        lambda b: b.referenced_items(),
    ):
        e = Env(lock_after=1)
        e.broker.unlock({"it": "s3cret-val"}, {})
        e.clock.now += 60
        call(e.broker)
        assert e.broker.locked


def test_handle_run_after_idle_asks_locked() -> None:
    e = Env(lock_after=1)
    e.broker.unlock({"it": "s3cret-val"}, {})
    e.clock.now += 60
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.broker.handle_run(req())
    assert e.asked[0].locked is True


def test_use_resets_idle_timer() -> None:
    e = Env(lock_after=1)
    e.broker.unlock({"it": "s3cret-val"}, {})
    e.clock.now += 50
    e.broker.handle_run(req())  # asks (first use), once -> runs, last_use = now
    e.clock.now += 50
    e.broker.tick()
    assert not e.broker.locked
    e.clock.now += 10
    e.broker.tick()
    assert e.broker.locked


def test_unlock_sets_last_use() -> None:
    e = Env(lock_after=1)
    e.clock.now = 5000
    e.broker.unlock({"it": "s3cret-val"}, {})
    e.clock.now = 5059
    e.broker.tick()
    assert not e.broker.locked
    e.clock.now = 5060
    e.broker.tick()
    assert e.broker.locked


def test_idle_lock_clears_grant() -> None:
    e = Env(lock_after=1)
    e.broker.unlock({"it": "s3cret-val"}, {})
    e.answers = ["grant"]
    e.broker.handle_run(req())
    e.clock.now += 60
    e.broker.tick()
    assert e.broker.locked
    assert e.broker.list_actions()[0].state == "locked"


def test_timed_out_is_failed() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.result = RunResult(137, b"", b"", timed_out=True)
    assert e.broker.handle_run(req()) == Failed("action timed out after 30 s")


def test_timed_out_counts_as_use() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.result = RunResult(137, b"", b"", timed_out=True)
    e.broker.handle_run(req())
    e.broker.handle_run(req())
    assert [p.first_use for p in e.asked] == [True, False]


def test_output_cap() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.result = RunResult(0, b"a" * CAP_MIB5, b"b" * 10)
    r = e.broker.handle_run(req())
    assert isinstance(r, Ran)
    assert len(r.stdout) == OUTPUT_CAP
    assert r.stderr == b"b" * 10
    assert r.truncated is True


def test_stderr_cap() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.result = RunResult(0, b"", b"c" * CAP_MIB5)
    r = e.broker.handle_run(req())
    assert isinstance(r, Ran)
    assert len(r.stderr) == OUTPUT_CAP
    assert r.truncated is True


def test_exactly_cap_not_truncated() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.result = RunResult(0, b"a" * OUTPUT_CAP, b"")
    r = e.broker.handle_run(req())
    assert isinstance(r, Ran)
    assert len(r.stdout) == OUTPUT_CAP
    assert r.truncated is False


def test_overflow_flag_truncates() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "s3cret-val"})
    e.result = RunResult(0, b"small-output", b"", overflow=True)
    r = e.broker.handle_run(req())
    assert isinstance(r, Ran)
    assert r.truncated is True
    # an overflowed stream loses its last len(secret) - 1 bytes
    assert r.stdout == b"small-output"[: -(len("s3cret-val") - 1)]


def test_short_value_not_masked() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "abc12"})
    e.result = RunResult(0, b"v=abc12", b"")
    assert e.broker.handle_run(req()) == Ran(0, b"v=abc12", b"", False)


def test_only_own_items_masked() -> None:
    e = Env({"a": make_action("a", secrets={"A": "ia"}), "b": make_action("b", secrets={"B": "ib"})})
    e.unlock_on_next_ask({"ia": "valueAAA", "ib": "valueBBB"})
    e.result = RunResult(0, b"valueAAA valueBBB", b"")
    assert e.broker.handle_run(req("a")) == Ran(0, b"[masked] valueBBB", b"", False)


def test_utf8_value_masked() -> None:
    e = Env()
    e.unlock_on_next_ask({"it": "zażółć-gęślą"})
    e.result = RunResult(0, "x zażółć-gęślą".encode(), b"")
    assert e.broker.handle_run(req()) == Ran(0, b"x [masked]", b"", False)


def test_list_actions_sorted_and_info() -> None:
    e = Env(
        {
            "zed": make_action("zed", args=("^a$", "^b+$"), secrets={"X": "i2", "Y": "i1", "Z": "i2"}),
            "alpha": make_action("alpha"),
        }
    )
    infos = e.broker.list_actions()
    assert [i.name for i in infos] == ["alpha", "zed"]
    assert infos[0] == ActionInfo("alpha", "desc alpha", (), (), "locked", "")
    assert infos[1] == ActionInfo("zed", "desc zed", ("^a$", "^b+$"), ("i1", "i2"), "locked", "")


def test_list_actions_states() -> None:
    e = Env(
        {
            "ok": make_action("ok", secrets={"A": "ia"}),
            "bad": make_action("bad", secrets={"B": "ib"}),
        }
    )
    assert [(i.state, i.detail) for i in e.broker.list_actions()] == [("locked", ""), ("locked", "")]
    e.broker.unlock({"ia": "valueA1"}, {"ib": "item missing: ib"})
    assert [(i.name, i.state, i.detail) for i in e.broker.list_actions()] == [
        ("bad", "unavailable", "item missing: ib"),
        ("ok", "ready", ""),
    ]
    e.answers = ["grant"]
    e.broker.handle_run(req("ok"))
    t0 = e.clock.now
    assert e.broker.list_actions()[1].state == "granted"
    assert e.broker.list_actions()[1].detail == "15 min left"
    e.clock.now = t0 + 1
    assert e.broker.list_actions()[1].detail == "15 min left"
    e.clock.now = t0 + 60
    assert e.broker.list_actions()[1].detail == "14 min left"
    e.clock.now = t0 + 839
    assert e.broker.list_actions()[1].detail == "2 min left"
    e.clock.now = t0 + 840
    assert e.broker.list_actions()[1].detail == "1 min left"
    e.clock.now = t0 + 899.5
    assert e.broker.list_actions()[1].detail == "1 min left"
    e.clock.now = t0 + 900
    assert e.broker.list_actions()[1].state == "ready"
    assert e.broker.list_actions()[1].detail == ""


def test_list_actions_locked_hides_item_errors() -> None:
    e = Env()
    e.broker.unlock({}, {"it": "item missing: it"})
    e.broker.lock()
    assert e.broker.list_actions()[0].state == "locked"


@pytest.mark.parametrize("answer", ["deny", "timeout"])
def test_denied_requests_do_not_unlock_state(answer: Answer) -> None:
    e = Env()
    e.answers = [answer]
    e.broker.handle_run(req())
    assert e.broker.locked


def test_overflow_does_not_leak_partial_secret_prefix() -> None:
    secret = "Zs3cret-value-XYZ12"
    sb = secret.encode()
    e = Env()
    e.unlock_on_next_ask({"it": secret})
    n = OUTPUT_CAP // len(sb) + 100
    raw = sb * n + sb[:-1]
    assert len(raw) > OUTPUT_CAP
    e.result = RunResult(0, raw, b"", overflow=True)
    r = e.broker.handle_run(req())
    assert isinstance(r, Ran)
    assert r.truncated is True
    assert len(r.stdout) < OUTPUT_CAP
    assert sb not in r.stdout
    for k in range(1, len(sb)):
        assert not r.stdout.endswith(sb[:k]), k
