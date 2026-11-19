import json

from mail_hook.app import PublishError, handle

TOKEN = "s3cret"
AUTH = {"Authorization": f"Bearer {TOKEN}"}
MAIL = {
    "kind": "hook",
    "from": "a@example.com",
    "to": "b@example.com",
    "subject": "hi",
    "snippet": "hello there",
}


class Recorder:
    def __init__(self, fail: bool = False) -> None:
        self.calls: list[tuple[str, str]] = []
        self.fail = fail

    def __call__(self, subject: str, line: str) -> None:
        if self.fail:
            raise PublishError("down")
        self.calls.append((subject, line))


def call(
    rec: Recorder,
    *,
    method: str = "POST",
    path: str = "/mail",
    headers: dict[str, str] | None = None,
    body: bytes | None = None,
) -> int:
    return handle(
        method,
        path,
        AUTH if headers is None else headers,
        json.dumps(MAIL).encode() if body is None else body,
        TOKEN,
        rec,
    )


def test_wrong_path_is_404() -> None:
    rec = Recorder()
    assert call(rec, path="/other") == 404
    assert rec.calls == []


def test_wrong_method_is_405() -> None:
    rec = Recorder()
    assert call(rec, method="GET") == 405
    assert rec.calls == []


def test_missing_authorization_is_401() -> None:
    rec = Recorder()
    assert call(rec, headers={}) == 401
    assert rec.calls == []


def test_wrong_token_is_401() -> None:
    rec = Recorder()
    assert call(rec, headers={"Authorization": "Bearer nope"}) == 401
    assert call(rec, headers={"Authorization": TOKEN}) == 401
    assert rec.calls == []


def test_header_lookup_is_case_insensitive() -> None:
    rec = Recorder()
    assert call(rec, headers={"authorization": f"Bearer {TOKEN}"}) == 204
    assert call(rec, headers={"AUTHORIZATION": f"Bearer {TOKEN}"}) == 204


def test_oversized_body_is_413() -> None:
    rec = Recorder()
    assert call(rec, body=b" " * 65537) == 413
    assert rec.calls == []


def test_body_at_limit_is_not_413() -> None:
    rec = Recorder()
    body = json.dumps(MAIL).encode().ljust(65536, b" ")
    assert call(rec, body=body) == 204


def test_invalid_json_is_400() -> None:
    rec = Recorder()
    assert call(rec, body=b"not json") == 400
    assert rec.calls == []


def test_non_object_json_is_400() -> None:
    rec = Recorder()
    assert call(rec, body=b"[1, 2]") == 400
    assert rec.calls == []


def test_unknown_kind_is_400() -> None:
    rec = Recorder()
    assert call(rec, body=json.dumps({**MAIL, "kind": "spam"}).encode()) == 400
    assert rec.calls == []


def test_missing_field_is_400() -> None:
    rec = Recorder()
    body = {k: v for k, v in MAIL.items() if k != "snippet"}
    assert call(rec, body=json.dumps(body).encode()) == 400
    assert rec.calls == []


def test_non_string_field_is_400() -> None:
    rec = Recorder()
    assert call(rec, body=json.dumps({**MAIL, "subject": 5}).encode()) == 400
    assert rec.calls == []


def test_publish_error_is_502() -> None:
    assert call(Recorder(fail=True)) == 502


def test_success_publishes_one_line_and_returns_204() -> None:
    rec = Recorder()
    assert call(rec, body=json.dumps({**MAIL, "kind": "throwaway", "extra": "x"}).encode()) == 204
    assert len(rec.calls) == 1
    subject, line = rec.calls[0]
    assert subject == "mail.received"
    assert "\n" not in line
    assert json.loads(line) == {
        "type": "mail.received",
        "kind": "throwaway",
        "from": MAIL["from"],
        "to": MAIL["to"],
        "subject": MAIL["subject"],
        "snippet": MAIL["snippet"],
    }


def test_first_matching_rule_wins() -> None:
    rec = Recorder()
    assert call(rec, path="/other", method="GET", headers={}, body=b"x" * 70000) == 404
    assert call(rec, method="GET", headers={}, body=b"x" * 70000) == 405
    assert call(rec, headers={}, body=b"x" * 70000) == 401
    assert call(rec, body=b"x" * 70000) == 413
