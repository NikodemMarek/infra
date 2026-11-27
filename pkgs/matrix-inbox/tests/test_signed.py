import base64

import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import (
    Ed25519PrivateKey,
    Ed25519PublicKey,
)
from matrix_inbox.signed import load_key


def raw_public(key: Ed25519PrivateKey) -> bytes:
    return key.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    )


def test_load_key_roundtrip() -> None:
    key = Ed25519PrivateKey.generate()
    loaded = load_key(base64.b64encode(raw_public(key)).decode())
    assert isinstance(loaded, Ed25519PublicKey)
    assert raw_public(key) == loaded.public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    )


@pytest.mark.parametrize(
    "text",
    [
        "!!!not base64",
        "AAAA",
        "",
        base64.b64encode(b"x" * 31).decode(),
        base64.b64encode(b"x" * 33).decode(),
    ],
)
def test_load_key_invalid(text: str) -> None:
    with pytest.raises(ValueError):
        load_key(text)
