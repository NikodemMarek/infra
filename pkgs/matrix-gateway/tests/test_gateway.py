import base64
import json

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from fakes import OWNER, ROOM, USER, FakeBus, FakeMatrix, owner_message
from matrix_gateway.data import Obj
from matrix_gateway.gateway import DEVICE_ID, Fatal, Gateway, MatrixError
from matrix_gateway.logic import ROUTE

NOT_ROUTED = "Reply to a message that asks for an answer, or send !<host> <task> as a new message."
BUS_DOWN = "Failed: message bus unavailable."


class Rig:
    def __init__(self, aliases: dict[str, str] | None = None) -> None:
        self.matrix = FakeMatrix(aliases={"#agents:nkmrk.com": ROOM} if aliases is None else aliases)
        self.bus = FakeBus()
        self.key = Ed25519PrivateKey.generate()
        self.gw = Gateway(self.matrix, self.bus, OWNER, "agents", "pw", self.key)

    def started(self) -> "Rig":
        self.gw.start()
        return self

    def routed(self, event_id: str = "$bot1", to: str = "yenn", ref: str = "r", body: str = "B") -> None:
        self.matrix.add_event(event_id, USER, {"msgtype": "m.text", "body": body, ROUTE: {"to": to, "ref": ref}})


def verify(rig: Rig, data: str) -> Obj:
    env = json.loads(data)
    assert env["type"] == "matrix.reply"
    rig.key.public_key().verify(base64.b64decode(env["sig"]), env["msg"].encode("utf-8"))
    msg = json.loads(env["msg"])
    assert isinstance(msg, dict)
    return msg


def test_device_id() -> None:
    assert DEVICE_ID == "matrix-gateway"


def test_start_logs_in_and_uses_existing_room() -> None:
    r = Rig().started()
    assert r.matrix.logins == [("agents", "pw", "matrix-gateway")]
    assert r.matrix.resolved == ["#agents:nkmrk.com"]
    assert r.matrix.created == []
    assert r.matrix.sync_calls == [(ROOM, None, 0)]


def test_start_creates_room() -> None:
    r = Rig(aliases={}).started()
    assert r.matrix.created == [("agents", OWNER)]
    assert r.matrix.sync_calls[0][0] == r.matrix.created_room


def test_start_alias_server_from_user_id() -> None:
    r = Rig(aliases={"#agents:other.org": ROOM})
    r.matrix.user = "@agents:other.org"
    r.gw.start()
    assert r.matrix.resolved == ["#agents:other.org"]
    assert r.matrix.created == []


def test_start_ignores_backlog() -> None:
    r = Rig()
    r.routed()
    r.matrix.script = [("s1", [owner_message("$o", "y", parent="$bot1")])]
    r.gw.start()
    assert r.bus.published == []
    assert r.matrix.sent == []


def test_step_syncs_from_stored_since() -> None:
    r = Rig()
    r.matrix.script = [("s1", []), ("s2", []), ("s3", [])]
    r.gw.start()
    r.gw.step()
    r.gw.step()
    assert r.matrix.sync_calls[1] == (ROOM, "s1", 30000)
    assert r.matrix.sync_calls[2] == (ROOM, "s2", 30000)


def test_on_send_routed() -> None:
    r = Rig().started()
    r.gw.on_send('{"to":"yenn","ref":"/r/x","body":"hi"}', "t1")
    assert len(r.matrix.sent) == 1
    s = r.matrix.sent[0]
    assert s.room == ROOM
    assert s.txn == "t1"
    assert s.event_type == "m.room.message"
    assert s.content == {"msgtype": "m.text", "body": "hi", ROUTE: {"to": "yenn", "ref": "/r/x"}}


def test_on_send_notice() -> None:
    r = Rig().started()
    r.gw.on_send('{"reply_to":"$e1","body":"ok"}', "t2")
    assert len(r.matrix.sent) == 1
    assert r.matrix.sent[0].txn == "t2"
    assert r.matrix.sent[0].content == {
        "msgtype": "m.notice",
        "body": "ok",
        "m.relates_to": {"m.in_reply_to": {"event_id": "$e1"}},
    }


def test_on_send_propagates_matrix_error() -> None:
    r = Rig().started()
    r.matrix.send_failures = [MatrixError("429")]
    with pytest.raises(MatrixError):
        r.gw.on_send('{"to":"yenn","ref":"r","body":"hi"}', "t1")
    assert r.matrix.send_attempts == ["t1"]
    assert r.matrix.sent == []


@pytest.mark.parametrize("line", ['{"body":"hi"}', "nil", "[1]", '{"to":"Y","ref":"r","body":"x"}', '{"body":""}'])
def test_on_send_invalid(line: str) -> None:
    r = Rig().started()
    r.gw.on_send(line, "t")
    assert r.matrix.sent == []


def test_reply_published_signed() -> None:
    r = Rig().started()
    r.routed(to="yenn", ref="r", body="B")
    r.gw.on_event(owner_message("$o1", "y", parent="$bot1"))
    assert len(r.bus.published) == 1
    subject, data = r.bus.published[0]
    assert subject == "matrix.reply.yenn"
    assert verify(r, data) == {"to": "yenn", "ref": "r", "text": "y", "body": "B", "event_id": "$o1"}
    assert r.matrix.sent == []


def test_reply_fallback_stripped() -> None:
    r = Rig().started()
    r.routed()
    r.gw.on_event(owner_message("$o1", "> <@agents:nkmrk.com> B\n\n  y ", parent="$bot1"))
    assert verify(r, r.bus.published[0][1])["text"] == "y"


def test_parent_without_body_gives_empty_body() -> None:
    r = Rig().started()
    r.matrix.add_event("$bot1", USER, {ROUTE: {"to": "yenn", "ref": "r"}})
    r.gw.on_event(owner_message("$o1", "y", parent="$bot1"))
    assert verify(r, r.bus.published[0][1])["body"] == ""


@pytest.mark.parametrize(
    "ev",
    [
        owner_message("$o", "y", parent="$bot1", sender="@mallory:nkmrk.com"),
        owner_message("$o", "y", parent="$bot1", msgtype="m.notice"),
        owner_message("$o", "y", parent="$bot1", rel_type="m.replace"),
        owner_message("$o", "y", parent="$bot1", event_type="m.reaction"),
    ],
)
def test_ignored_events(ev: Obj) -> None:
    r = Rig().started()
    r.routed()
    r.gw.on_event(ev)
    assert r.bus.published == []
    assert r.matrix.sent == []


def assert_not_routed_notice(r: Rig, event_id: str = "$o") -> None:
    assert r.bus.published == []
    assert len(r.matrix.sent) == 1
    s = r.matrix.sent[0]
    assert s.txn == "re-" + event_id
    assert s.content == {
        "msgtype": "m.notice",
        "body": NOT_ROUTED,
        "m.relates_to": {"m.in_reply_to": {"event_id": event_id}},
    }


def test_top_level_message() -> None:
    r = Rig().started()
    r.gw.on_event(owner_message("$o", "y"))
    assert_not_routed_notice(r)


def test_unknown_parent() -> None:
    r = Rig().started()
    r.gw.on_event(owner_message("$o", "y", parent="$gone"))
    assert_not_routed_notice(r)


def test_parent_by_other_sender() -> None:
    r = Rig().started()
    r.matrix.add_event("$p", "@mallory:nkmrk.com", {"body": "B", ROUTE: {"to": "yenn", "ref": "r"}})
    r.gw.on_event(owner_message("$o", "y", parent="$p"))
    assert_not_routed_notice(r)


def test_parent_without_route() -> None:
    r = Rig().started()
    r.matrix.add_event("$p", USER, {"msgtype": "m.text", "body": "B"})
    r.gw.on_event(owner_message("$o", "y", parent="$p"))
    assert_not_routed_notice(r)


def test_parent_route_invalid_to() -> None:
    r = Rig().started()
    r.routed(to="a.b")
    r.gw.on_event(owner_message("$o", "y", parent="$bot1"))
    assert_not_routed_notice(r)


def test_bus_error_notice() -> None:
    r = Rig().started()
    r.routed()
    r.bus.fail()
    r.gw.on_event(owner_message("$o", "y", parent="$bot1"))
    assert r.matrix.notices() == r.matrix.sent
    assert len(r.matrix.notices()) == 1
    s = r.matrix.notices()[0]
    assert s.txn == "re-$o"
    assert s.content["body"] == BUS_DOWN
    assert s.content["msgtype"] == "m.notice"
    assert s.content["m.relates_to"] == {"m.in_reply_to": {"event_id": "$o"}}


def test_matrix_error_on_one_event_does_not_stop_the_next() -> None:
    r = Rig()
    r.routed()
    r.matrix.event_errors["$bad"] = MatrixError("boom")
    r.matrix.script = [
        ("s1", []),
        ("s2", [owner_message("$o1", "n", parent="$bad"), owner_message("$o2", "y", parent="$bot1")]),
    ]
    r.gw.start()
    r.gw.step()
    assert len(r.bus.published) == 1
    assert verify(r, r.bus.published[0][1])["event_id"] == "$o2"


def test_step_stores_since_before_handling() -> None:
    r = Rig()
    r.matrix.event_errors["$bad"] = MatrixError("boom")
    r.matrix.script = [("s1", []), ("s2", [owner_message("$o1", "n", parent="$bad")])]
    r.gw.start()
    r.gw.step()
    r.gw.step()
    assert r.matrix.sync_calls[-1][1] == "s2"


def test_fatal_propagates() -> None:
    r = Rig()
    r.matrix.event_errors["$bad"] = Fatal("401")
    r.matrix.script = [("s1", []), ("s2", [owner_message("$o", "y", parent="$bad")])]
    r.gw.start()
    with pytest.raises(Fatal):
        r.gw.step()
    with pytest.raises(Fatal):
        r.gw.on_event(owner_message("$o", "y", parent="$bad"))


def verify_command(rig: Rig, data: str) -> Obj:
    env = json.loads(data)
    assert env["type"] == "matrix.command"
    rig.key.public_key().verify(base64.b64decode(env["sig"]), env["msg"].encode("utf-8"))
    msg = json.loads(env["msg"])
    assert isinstance(msg, dict)
    return msg


def command_event(
    event_id: str,
    body: str,
    ts: int | None = 1700000000000,
    parent: str | None = None,
    sender: str = OWNER,
    msgtype: str = "m.text",
    rel_type: str | None = None,
    event_type: str = "m.room.message",
) -> Obj:
    ev = owner_message(event_id, body, parent, sender, msgtype, rel_type, event_type)
    if ts is not None:
        ev["origin_server_ts"] = ts
    return ev


def test_command_published_signed() -> None:
    r = Rig().started()
    r.gw.on_event(command_event("$c1", "!yenn fix it"))
    assert len(r.bus.published) == 1
    subject, data = r.bus.published[0]
    assert subject == "matrix.command.yenn"
    assert verify_command(r, data) == {
        "kind": "matrix.command",
        "to": "yenn",
        "text": "fix it",
        "event_id": "$c1",
        "ts": 1700000000000,
    }
    assert r.matrix.sent == []


def test_command_host_lowercased() -> None:
    r = Rig().started()
    r.gw.on_event(command_event("$c1", "!Yenn x"))
    assert [s for s, _ in r.bus.published] == ["matrix.command.yenn"]
    assert verify_command(r, r.bus.published[0][1])["to"] == "yenn"


@pytest.mark.parametrize(
    "ev",
    [
        command_event("$c", "!yenn x", sender="@mallory:nkmrk.com"),
        command_event("$c", "!yenn x", msgtype="m.notice"),
        command_event("$c", "!yenn x", rel_type="m.replace"),
        command_event("$c", "!yenn x", event_type="m.reaction"),
    ],
)
def test_command_ignored_events(ev: Obj) -> None:
    r = Rig().started()
    r.gw.on_event(ev)
    assert r.bus.published == []
    assert r.matrix.sent == []


def test_command_without_timestamp() -> None:
    r = Rig().started()
    r.gw.on_event(command_event("$c", "!yenn x", ts=None))
    assert r.bus.published == []
    assert r.matrix.sent == []


@pytest.mark.parametrize("value", [True, False, "1700000000000", 1.5])
def test_command_with_non_int_timestamp(value: bool | str | float) -> None:
    r = Rig().started()
    ev = command_event("$c", "!yenn x", ts=None)
    ev["origin_server_ts"] = value
    r.gw.on_event(ev)
    assert r.bus.published == []
    assert r.matrix.sent == []


def test_command_bus_error_notice() -> None:
    r = Rig().started()
    r.bus.fail()
    r.gw.on_event(command_event("$c", "!yenn x"))
    assert len(r.matrix.sent) == 1
    s = r.matrix.sent[0]
    assert s.txn == "re-$c"
    assert s.content == {
        "msgtype": "m.notice",
        "body": BUS_DOWN,
        "m.relates_to": {"m.in_reply_to": {"event_id": "$c"}},
    }


@pytest.mark.parametrize("body", ["hello", "!yenn", "!yenn   "])
def test_top_level_non_command_notice(body: str) -> None:
    r = Rig().started()
    r.gw.on_event(command_event("$o", body))
    assert_not_routed_notice(r)


def test_reply_with_command_body_and_unknown_parent_is_not_a_command() -> None:
    r = Rig().started()
    r.gw.on_event(command_event("$o", "!yenn x", parent="$gone"))
    assert_not_routed_notice(r)


def test_reply_starting_with_bang_stays_an_answer() -> None:
    r = Rig().started()
    r.routed(to="yenn", ref="r", body="B")
    r.gw.on_event(command_event("$o1", "!yenn x", parent="$bot1"))
    assert len(r.bus.published) == 1
    subject, data = r.bus.published[0]
    assert subject == "matrix.reply.yenn"
    assert verify(r, data)["text"] == "!yenn x"
    assert r.matrix.sent == []
