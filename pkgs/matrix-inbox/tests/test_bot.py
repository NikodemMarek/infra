import pytest

from fakes import (
    OWNER,
    PERMISSION,
    QUESTION,
    ROOM,
    USER,
    FakeLocal,
    FakeMatrix,
    bot_message,
    item,
    owner_message,
    permission,
    question,
)
from matrix_inbox import logic
from matrix_inbox.bot import Bot, Fatal, LocalError, MatrixError
from matrix_inbox.data import Obj

META = logic.META
ALLOW = '{"behavior":"allow"}'
DENY = '{"behavior":"deny"}'
GONE = "No longer waiting: answered elsewhere or expired."
NOT_A_REPLY = "Reply to an item to answer it."
CLAIMED = "Refusing: another item claims the same reply target."


def make(items: list[Obj] | None = None, aliases: dict[str, str] | None = None) -> tuple[Bot, FakeMatrix, FakeLocal]:
    matrix = FakeMatrix(aliases={"#agents:nkmrk.com": ROOM} if aliases is None else aliases)
    local = FakeLocal(items)
    return Bot(matrix, local, OWNER), matrix, local


def started(items: list[Obj] | None = None) -> tuple[Bot, FakeMatrix, FakeLocal]:
    bot, matrix, local = make(items)
    bot.start()
    return bot, matrix, local


def notice(reply_to: str, body: str) -> Obj:
    return {"msgtype": "m.notice", "body": body, "m.relates_to": {"m.in_reply_to": {"event_id": reply_to}}}


def reaction(event_id: str) -> Obj:
    return {"m.relates_to": {"rel_type": "m.annotation", "event_id": event_id, "key": "✅"}}


def deliver(matrix: FakeMatrix, *events: Obj) -> None:
    matrix.script.append(("next", list(events)))


def answer(bot: Bot, matrix: FakeMatrix, parent: str, body: str, event_id: str = "$e1") -> None:
    deliver(matrix, owner_message(event_id, body, parent=parent))
    bot.step()


def bot_parent(matrix: FakeMatrix, item_id: str, kind: str, event_id: str = "$p", sender: str = USER) -> str:
    matrix.add_event(event_id, sender, bot_message(item_id, kind))
    return event_id


# start


def test_start_creates_room_when_alias_unresolved() -> None:
    bot, matrix, _ = make(aliases={})
    bot.start()
    assert matrix.resolved == ["#agents:nkmrk.com"]
    assert matrix.created == [("agents", OWNER)]
    assert matrix.sync_calls[0][0] == matrix.created_room


def test_start_uses_resolved_room() -> None:
    bot, matrix, _ = make()
    bot.start()
    assert matrix.created == []
    assert matrix.sync_calls[0][0] == ROOM
    assert matrix.recent_calls == [(ROOM, USER, 100)]


def test_start_alias_uses_server_part_after_first_colon() -> None:
    matrix = FakeMatrix(user="@agents:example.org:8448", aliases={"#agents:example.org:8448": ROOM})
    Bot(matrix, FakeLocal(), OWNER).start()
    assert matrix.resolved == ["#agents:example.org:8448"]
    assert matrix.created == []


def test_start_first_sync_is_nonblocking_and_its_events_ignored() -> None:
    bot, matrix, local = make([item("A", permission())])
    matrix.script.append(("s0", [owner_message("$old", "y", parent="$p")]))
    bot_parent(matrix, "A", PERMISSION)
    bot.start()
    assert matrix.sync_calls == [(ROOM, None, 0)]
    assert matrix.sent == []
    assert local.replies == []
    bot.step()
    assert matrix.sync_calls[1] == (ROOM, "s0", 5000)
    assert local.replies == []


def test_start_adopts_ids_posted_in_the_last_messages() -> None:
    bot, matrix, local = make([item("A", permission(reply="/r/a")), item("B", permission(reply="/r/b"))])
    matrix.recent = [
        {"type": "m.room.message", "sender": USER, "event_id": "$1", "content": bot_message("A", PERMISSION)},
        {"type": "m.room.message", "sender": USER, "event_id": "$2", "content": {"msgtype": "m.text", "body": "x"}},
        {"type": "m.room.message", "sender": USER, "event_id": "$3", "content": {"msgtype": "m.text", META: "junk"}},
        {"type": "m.room.message", "sender": USER, "event_id": "$4", "content": {"msgtype": "m.text", META: {"id": 5}}},
    ]
    bot.start()
    bot.step()
    assert [p.txn for p in matrix.posts()] == ["inbox-B"]
    assert local.replies == []


# step: mirror


def test_step_posts_offered_item_once() -> None:
    p = item("A", permission())
    bot, matrix, _ = started([p])
    bot.step()
    bot.step()
    assert len(matrix.sent) == 1
    sent = matrix.sent[0]
    assert (sent.room, sent.txn, sent.event_type) == (ROOM, "inbox-A", "m.room.message")
    assert sent.content == logic.render(p)
    assert sent.content[META] == {"id": "A", "kind": PERMISSION}


def test_step_posts_in_order() -> None:
    bot, matrix, _ = started([item("A", permission(reply="/r/a")), item("B", question(reply="/r/b"))])
    bot.step()
    assert [p.txn for p in matrix.posts()] == ["inbox-A", "inbox-B"]


def test_step_posts_only_new_items() -> None:
    local_items = [item("A", permission(reply="/r/a"))]
    bot, matrix, local = started(local_items)
    bot.step()
    local.held = [item("B", question(reply="/r/b"))]
    bot.step()
    local.held = [item("B", question(reply="/r/b")), item("C", question(reply="/r/c"))]
    bot.step()
    assert [p.txn for p in matrix.posts()] == ["inbox-A", "inbox-B", "inbox-C"]


def test_step_does_not_post_unanswerable_items() -> None:
    items = [
        item("S", {"type": "sudo.password_request", "reply": "/r/s"}),
        item("W", {"type": "claude.waiting", "reply": "/r/w"}),
        item("T", {"type": "task.failed", "reply": "/r/t"}),
        item("U", {"type": "mystery", "reply": "/r/u"}),
        item("Q", question(reply=None)),
        item("E", question(reply="")),
        item("P", permission(reply=None)),
    ]
    bot, matrix, _ = started(items)
    bot.step()
    assert matrix.sent == []


def test_step_does_not_post_item_reusing_an_earlier_reply_path() -> None:
    bot, matrix, _ = started([item("A", permission(reply="/r/x")), item("B", question(reply="/r/x"))])
    bot.step()
    assert [p.txn for p in matrix.posts()] == ["inbox-A"]


def test_step_posts_item_reusing_a_reply_path_once_the_earlier_item_is_gone() -> None:
    bot, matrix, local = started([item("A", permission(reply="/r/x"))])
    bot.step()
    local.held = [item("B", question(reply="/r/x"))]
    bot.step()
    assert [p.txn for p in matrix.posts()] == ["inbox-A", "inbox-B"]


def test_step_reposts_nothing_while_item_stays_and_reposts_after_it_left() -> None:
    bot, matrix, local = started([item("A", permission())])
    bot.step()
    held = local.held
    local.held = []
    bot.step()
    local.held = held
    bot.step()
    assert [p.txn for p in matrix.posts()] == ["inbox-A", "inbox-A"]


def test_step_send_failure_is_retried_next_step() -> None:
    bot, matrix, _ = started([item("A", permission(reply="/r/a")), item("B", permission(reply="/r/b"))])
    matrix.send_errors["inbox-A"] = MatrixError("boom")
    bot.step()
    assert [p.txn for p in matrix.posts()] == ["inbox-B"]
    del matrix.send_errors["inbox-A"]
    bot.step()
    assert [p.txn for p in matrix.posts()] == ["inbox-B", "inbox-A"]


def test_step_sync_matrix_error_propagates() -> None:
    bot, matrix, _ = started()
    matrix.sync_error = MatrixError("down")
    with pytest.raises(MatrixError):
        bot.step()


def test_step_sync_fatal_propagates() -> None:
    bot, matrix, _ = started()
    matrix.sync_error = Fatal("401")
    with pytest.raises(Fatal):
        bot.step()


def test_step_send_fatal_propagates() -> None:
    bot, matrix, _ = started([item("A", permission())])
    matrix.send_errors["inbox-A"] = Fatal("401")
    with pytest.raises(Fatal):
        bot.step()


# step: sync token


def test_step_syncs_with_the_stored_token() -> None:
    bot, matrix, _ = started()
    matrix.script = [("t1", []), ("t2", [])]
    bot.step()
    bot.step()
    assert matrix.sync_calls[1:] == [(ROOM, "s1", 5000), (ROOM, "t1", 5000)]
    bot.step()
    assert matrix.sync_calls[3] == (ROOM, "t2", 5000)


def test_step_stores_token_before_handling_events() -> None:
    bot, matrix, local = started([item("A", permission())])
    bot_parent(matrix, "A", PERMISSION)
    matrix.script = [("t1", [owner_message("$e1", "y", parent="$p")])]
    matrix.send_errors["ok-$e1"] = Fatal("401")
    with pytest.raises(Fatal):
        bot.step()
    del matrix.send_errors["ok-$e1"]
    bot.step()
    assert matrix.sync_calls[-1] == (ROOM, "t1", 5000)
    assert local.replies == [("A", ALLOW)]


def test_event_matrix_error_does_not_stop_next_events() -> None:
    bot, matrix, local = started([item("A", permission(reply="/r/a")), item("B", question(reply="/r/b"))])
    bot_parent(matrix, "A", PERMISSION, "$pa")
    bot_parent(matrix, "B", QUESTION, "$pb")
    matrix.send_errors["ok-$e1"] = MatrixError("boom")
    matrix.script = [("t1", [owner_message("$e1", "y", parent="$pa"), owner_message("$e2", "1", parent="$pb")])]
    bot.step()
    assert local.replies == [("A", ALLOW), ("B", '{"behavior":"allow","answers":{"Which db?":"A"}}')]
    assert [r.txn for r in matrix.reactions()] == ["ok-$e2"]


def test_event_unreadable_inbox_notice_does_not_stop_next_events() -> None:
    bot, matrix, local = started()
    bot_parent(matrix, "A", PERMISSION)
    local.items_error = LocalError("boom")
    matrix.script = [("t1", [owner_message("$e1", "y", parent="$p"), owner_message("$e2", "hello")])]
    with pytest.raises(LocalError):
        bot.step()
    assert [n.txn for n in matrix.notices()] == ["re-$e1", "re-$e2"]


def test_event_fatal_propagates() -> None:
    bot, matrix, _ = started()
    matrix.script = [("t1", [owner_message("$e1", "hello")])]
    matrix.send_errors["re-$e1"] = Fatal("401")
    with pytest.raises(Fatal):
        bot.step()


# on_event: answers


def test_owner_y_approves_permission() -> None:
    bot, matrix, local = started([item("A", permission())])
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "y")
    assert local.replies == [("A", ALLOW)]
    ok = matrix.reactions()
    assert len(ok) == 1
    assert (ok[0].room, ok[0].txn, ok[0].event_type) == (ROOM, "ok-$e1", "m.reaction")
    assert ok[0].content == reaction("$e1")
    assert matrix.notices() == []


def test_owner_n_denies_permission() -> None:
    bot, matrix, local = started([item("A", permission())])
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "No")
    assert local.replies == [("A", DENY)]
    assert len(matrix.reactions()) == 1


def test_owner_number_answers_question() -> None:
    bot, matrix, local = started([item("Q", question())])
    bot_parent(matrix, "Q", QUESTION)
    answer(bot, matrix, "$p", "1")
    assert local.replies == [("Q", '{"behavior":"allow","answers":{"Which db?":"A"}}')]
    assert matrix.reactions()[0].content == reaction("$e1")


def test_owner_reply_strips_reply_fallback() -> None:
    bot, matrix, local = started([item("A", permission())])
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "> <@agents:nkmrk.com> posted\n> more\n\n y \n")
    assert local.replies == [("A", ALLOW)]


def test_round_trip_through_posted_message() -> None:
    bot, matrix, local = started([item("A", permission())])
    bot.step()
    posted = matrix.posts()[0].event_id
    answer(bot, matrix, posted, "y")
    assert local.replies == [("A", ALLOW)]
    assert [p.txn for p in matrix.posts()] == ["inbox-A"]


def test_reply_to_gone_item() -> None:
    bot, matrix, local = started([])
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "y")
    assert local.replies == []
    assert [(n.txn, n.content) for n in matrix.notices()] == [("re-$e1", notice("$e1", GONE))]
    assert matrix.reactions() == []


def test_reply_when_item_kind_differs() -> None:
    bot, matrix, local = started([item("A", question())])
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "y")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", GONE)]


def test_reply_when_parent_says_permission_but_item_is_sudo_with_reply() -> None:
    sudo: Obj = {"type": "sudo.password_request", "reply": "/r/s"}
    bot, matrix, local = started([item("A", sudo)])
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "y")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", GONE)]
    assert matrix.reactions() == []


def test_reply_when_item_no_longer_offered() -> None:
    bot, matrix, local = started([item("A", permission(reply=None))])
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "y")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", GONE)]


def test_reply_to_forged_claimant() -> None:
    bot, matrix, local = started([item("A", permission(reply="/r/x")), item("B", permission(reply="/r/x"))])
    bot_parent(matrix, "B", PERMISSION)
    answer(bot, matrix, "$p", "y")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", CLAIMED)]
    assert matrix.reactions() == []


def test_truncated_permission_allow_is_refused() -> None:
    bot, matrix, local = started([item("A", permission(command="x" * 2500))])
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "y")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [
        notice("$e1", "Too long to approve here: reply n to deny, or answer it on the computer.")
    ]
    assert matrix.reactions() == []


def test_truncated_permission_deny_is_sent() -> None:
    bot, matrix, local = started([item("A", permission(command="x" * 2500))])
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "n")
    assert local.replies == [("A", DENY)]
    assert len(matrix.reactions()) == 1


def test_permission_refusal_notice() -> None:
    bot, matrix, local = started([item("A", permission())])
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "maybe")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", "Reply y or n.")]


def test_question_refusal_notice() -> None:
    bot, matrix, local = started([item("Q", question())])
    bot_parent(matrix, "Q", QUESTION)
    answer(bot, matrix, "$p", "9")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", "Question 1: there is no option 9.")]
    assert matrix.reactions() == []


def test_local_reply_error_becomes_notice() -> None:
    bot, matrix, local = started([item("A", permission())])
    local.reply_results["A"] = "expired: nothing is waiting for the answer"
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "y")
    assert local.replies == [("A", ALLOW)]
    assert [(n.txn, n.content) for n in matrix.notices()] == [
        ("re-$e1", notice("$e1", "Failed: expired: nothing is waiting for the answer"))
    ]
    assert matrix.reactions() == []


def test_local_reply_error_is_sanitized_single_line_and_capped() -> None:
    bot, matrix, local = started([item("A", permission())])
    local.reply_results["A"] = "bad\nthing\x1b"
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "y")
    assert [n.content for n in matrix.notices()] == [notice("$e1", "Failed: bad thing�")]

    bot, matrix, local = started([item("A", permission())])
    local.reply_results["A"] = "x" * 400
    bot_parent(matrix, "A", PERMISSION)
    answer(bot, matrix, "$p", "y")
    assert [n.content for n in matrix.notices()] == [notice("$e1", "Failed: " + "x" * 300)]


def test_unreadable_inbox_notice() -> None:
    bot, matrix, local = started()
    bot_parent(matrix, "A", PERMISSION)
    local.items_error = LocalError("boom")
    deliver(matrix, owner_message("$e1", "y", parent="$p"))
    try:
        bot.step()
    except LocalError:
        pass
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", "Failed: cannot read the inbox.")]


# on_event: ignored and not-a-reply


@pytest.mark.parametrize(
    "event",
    [
        owner_message("$e1", "y", parent="$p", sender="@mallory:nkmrk.com"),
        owner_message("$e1", "y", parent="$p", sender=USER),
        owner_message("$e1", "y", parent="$p", msgtype="m.notice"),
        owner_message("$e1", "y", parent="$p", msgtype="m.image"),
        owner_message("$e1", "y", parent="$p", rel_type="m.replace"),
        owner_message("$e1", "y", parent="$p", event_type="m.reaction"),
        owner_message("$e1", "y", parent="$p", event_type="m.room.member"),
        owner_message("$e1", "y", parent="$p", event_type="m.room.encrypted"),
    ],
)
def test_ignored_events(event: Obj) -> None:
    bot, matrix, local = started([item("A", permission())])
    bot_parent(matrix, "A", PERMISSION)
    sent_before = len(matrix.posts())
    deliver(matrix, event)
    bot.step()
    assert local.replies == []
    assert matrix.notices() == []
    assert matrix.reactions() == []
    assert len(matrix.posts()) == sent_before + 1


def test_top_level_text_is_not_a_reply() -> None:
    bot, matrix, local = started([item("A", permission())])
    deliver(matrix, owner_message("$e1", "y"))
    bot.step()
    assert local.replies == []
    assert [(n.txn, n.content) for n in matrix.notices()] == [("re-$e1", notice("$e1", NOT_A_REPLY))]


def test_reply_to_message_of_someone_else() -> None:
    bot, matrix, local = started([item("A", permission())])
    bot_parent(matrix, "A", PERMISSION, sender="@mallory:nkmrk.com")
    answer(bot, matrix, "$p", "y")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", NOT_A_REPLY)]


def test_reply_to_unknown_event() -> None:
    bot, matrix, local = started([item("A", permission())])
    answer(bot, matrix, "$missing", "y")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", NOT_A_REPLY)]


def test_reply_to_bot_message_without_meta() -> None:
    bot, matrix, local = started([item("A", permission())])
    matrix.add_event("$p", USER, {"msgtype": "m.notice", "body": "Failed: x"})
    answer(bot, matrix, "$p", "y")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", NOT_A_REPLY)]


@pytest.mark.parametrize(
    "meta",
    [
        {"id": "A"},
        {"kind": PERMISSION},
        {"id": 5, "kind": PERMISSION},
        {"id": "A", "kind": 5},
        {"id": "A", "kind": "claude.waiting"},
        {"id": "A", "kind": "sudo.password_request"},
    ],
)
def test_reply_to_bot_message_with_bad_meta(meta: Obj) -> None:
    bot, matrix, local = started([item("A", permission())])
    matrix.add_event("$p", USER, {"msgtype": "m.text", "body": "x", META: meta})
    answer(bot, matrix, "$p", "y")
    assert local.replies == []
    assert [n.content for n in matrix.notices()] == [notice("$e1", NOT_A_REPLY)]
