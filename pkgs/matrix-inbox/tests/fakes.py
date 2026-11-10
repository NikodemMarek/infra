from matrix_inbox.data import Json, Obj, loads

HOST = "yenn"
PERMISSION = "claude.permission_request"
QUESTION = "claude.question"


class FakeBus:
    """The `Bus` of the tests: records publishes, or raises `error` when set."""

    def __init__(self, error: Exception | None = None) -> None:
        self.error = error
        self.published: list[tuple[str, str]] = []

    def publish(self, subject: str, data: str) -> None:
        if self.error is not None:
            raise self.error
        self.published.append((subject, data))

    def payloads(self) -> list[Obj]:
        out: list[Obj] = []
        for _, data in self.published:
            value = loads(data)
            assert isinstance(value, dict)
            out.append(value)
        return out

    def notices(self) -> list[str]:
        """Bodies of the published notices; asserts each is a well-formed {reply_to, body} on matrix.send."""
        bodies: list[str] = []
        for (subject, _), payload in zip(self.published, self.payloads(), strict=True):
            assert subject == "matrix.send"
            assert set(payload) == {"reply_to", "body"}
            body = payload["body"]
            assert isinstance(body, str)
            bodies.append(body)
        return bodies


class FakeLocal:
    """The `Local` of the tests: holds the inbox items, records replies."""

    def __init__(self, items: list[Obj] | None = None) -> None:
        self.held: list[Obj] = list(items) if items is not None else []
        self.items_error: Exception | None = None
        self.replies: list[tuple[str, str]] = []
        self.reply_results: dict[str, str | None] = {}

    def items(self) -> list[Obj]:
        if self.items_error is not None:
            raise self.items_error
        return list(self.held)

    def reply(self, item_id: str, line: str) -> str | None:
        self.replies.append((item_id, line))
        return self.reply_results.get(item_id)


class FakeSeen:
    """The `Seen` of the tests: a set of claimed ids, or raises `error` when set."""

    def __init__(self, claimed: set[str] | None = None, error: OSError | None = None) -> None:
        self.claimed: set[str] = set(claimed) if claimed is not None else set()
        self.error = error
        self.calls: list[str] = []

    def claim(self, event_id: str) -> bool:
        self.calls.append(event_id)
        if self.error is not None:
            raise self.error
        if event_id in self.claimed:
            return False
        self.claimed.add(event_id)
        return True


class FakeTasks:
    """The `Tasks` of the tests: records the prompts, returns `outcome`."""

    def __init__(self, outcome: Obj | None = None) -> None:
        self.outcome = outcome
        self.prompts: list[str] = []

    def start(self, prompt: str) -> Obj | None:
        self.prompts.append(prompt)
        return self.outcome


def item(item_id: str, event: Obj, received: float = 1.0) -> Obj:
    return {"id": item_id, "received": received, "event": event}


def permission(reply: str | None = "/r/p", command: str = "ls -la", **extra: Json) -> Obj:
    event: Obj = {
        "type": PERMISSION,
        "project": "infra",
        "session": "s-1",
        "title": "List files",
        "tool_name": "Bash",
        "tool_input": {"command": command},
    }
    if reply is not None:
        event["reply"] = reply
    event.update(extra)
    return event


def question(reply: str | None = "/r/q", multi: bool = False, **extra: Json) -> Obj:
    q: Obj = {"question": "Which db?", "options": [{"label": "A"}, {"label": "B", "description": "fast"}, {"label": "C"}]}
    if multi:
        q["multiSelect"] = True
    event: Obj = {"type": QUESTION, "project": "infra", "tool_input": {"questions": [q]}}
    if reply is not None:
        event["reply"] = reply
    event.update(extra)
    return event
