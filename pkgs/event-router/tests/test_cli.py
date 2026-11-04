from event_router.cli import bus_down_event
from event_router.event import Event


def test_bus_down_event() -> None:
    e = bus_down_event("not connected to nats")
    assert isinstance(e, Event)
    assert e.type == "event_router.bus_down"
    assert e.data == {
        "type": "event_router.bus_down",
        "project": "event-router",
        "title": "event bus down: not connected to nats",
    }
