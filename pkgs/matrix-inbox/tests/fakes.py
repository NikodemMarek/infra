from matrix_inbox.data import Obj, loads

HOST = "yenn"


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


class FakeSeen:
    """The `Seen` of the tests: a set of claimed ids, or raises `error` when set."""

    def __init__(
        self, claimed: set[str] | None = None, error: OSError | None = None
    ) -> None:
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
