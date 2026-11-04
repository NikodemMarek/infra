import tomllib

import pytest

from secret_broker.data import Json
from secret_broker.model import Entry
from secret_broker.spec import SpecError, canonical_spec, resolved_titles

PROVIDER = "bw://?folder=infra/{profile}"


def canon(
    text: str,
    profiles: list[str],
    allowed: set[str] | None = None,
    provider: str = PROVIDER,
) -> tuple[str, dict[str, list[Entry]]]:
    return canonical_spec(text, profiles, allowed if allowed is not None else set(profiles), provider)


def reason(text: str, profiles: list[str], allowed: set[str] | None = None) -> str:
    with pytest.raises(SpecError) as e:
        canon(text, profiles, allowed)
    return str(e.value)


def expected_doc(provider: str, profiles: dict[str, dict[str, Json]]) -> dict[str, Json]:
    return {
        "project": {"name": "secret-broker-export", "revision": "1.0", "require_reason": False},
        "providers": {"bw": provider},
        "defaults": {"providers": ["bw"]},
        "profiles": profiles,
    }


def test_basic_render_and_entries() -> None:
    text = """
[project]
name = "x"

[profiles.p]
A = { description = "da", required = true, default = "dv" }
B = { composed = "${A}-x" }
C = {}
"""
    out, entries = canon(text, ["p"])
    assert tomllib.loads(out) == expected_doc(
        PROVIDER,
        {
            "p": {
                "A": {"description": "da", "required": True, "default": "dv"},
                "B": {"composed": "${A}-x"},
                "C": {},
            }
        },
    )
    assert entries == {
        "p": [
            Entry("A", description="da", required=True, default="dv"),
            Entry("B", composed="${A}-x"),
            Entry("C"),
        ]
    }


def test_required_false_rendered() -> None:
    out, entries = canon('[profiles.p]\nA = { required = false }', ["p"])
    assert tomllib.loads(out)["profiles"]["p"]["A"] == {"required": False}
    assert entries["p"] == [Entry("A", required=False)]


def test_field_order_in_text() -> None:
    out, _ = canon(
        '[profiles.p]\nA = { composed = "c", default = "d", required = true, description = "x" }',
        ["p"],
    )
    line = next(ln for ln in out.splitlines() if ln.startswith("A") or ln.startswith('"A"'))
    idx = [line.index(f) for f in ("description", "required", "default", "composed")]
    assert idx == sorted(idx)


def test_text_layout() -> None:
    out, _ = canon("[profiles.p]\nA = {}", ["p"])
    assert out.startswith('[project]\nname = "secret-broker-export"\nrevision = "1.0"\nrequire_reason = false\n\n[providers]\n')
    assert "\n\n[defaults]\nproviders = [\"bw\"]\n\n[profiles." in out


def test_entry_order_preserved() -> None:
    _, entries = canon("[profiles.p]\nZ = {}\nA = {}\nM = {}", ["p"])
    assert [e.key for e in entries["p"]] == ["Z", "A", "M"]


def test_multiple_profiles_dedupe_keep_order() -> None:
    text = "[profiles.b]\nB = {}\n[profiles.a]\nA = {}"
    out, entries = canon(text, ["b", "a", "b"])
    assert list(entries) == ["b", "a"]
    assert list(tomllib.loads(out)["profiles"]) == ["b", "a"]


def test_foreign_tables_ignored() -> None:
    text = """
[project]
name = "evil"
extends = ["/tmp/x"]

[providers]
bw = "bw://?folder="
other = "dotenv://x"

[defaults]
providers = ["other"]

[profiles.p]
A = { description = "d" }

[profiles.other]
B = { generate = true }

[profiles.defaults]
x = 1
"""
    out, entries = canon(text, ["p"])
    doc = tomllib.loads(out)
    assert doc == expected_doc(PROVIDER, {"p": {"A": {"description": "d"}}})
    assert "/tmp/x" not in out
    assert "dotenv" not in out
    assert "evil" not in out
    assert list(entries) == ["p"]


def test_provider_goes_to_output() -> None:
    out, _ = canon("[profiles.p]\nA = {}", ["p"], provider='bw://?folder="x"')
    assert tomllib.loads(out)["providers"] == {"bw": 'bw://?folder="x"'}


@pytest.mark.parametrize(
    "entry",
    [
        'K = { ref = {item = "x"} }',
        'K = { providers = ["x"] }',
        "K = { as_path = true }",
        "K = { generate = true }",
        'K = { type = "command" }',
    ],
)
def test_disallowed_fields(entry: str) -> None:
    r = reason(f"[profiles.p]\n{entry}", ["p"])
    assert r.startswith("spec rejected: p.K.")
    assert r.endswith(" is not allowed")


def test_disallowed_field_message_names_field() -> None:
    assert reason("[profiles.p]\nK = { generate = true }", ["p"]) == "spec rejected: p.K.generate is not allowed"


def test_defaults_entry_rejected() -> None:
    assert reason("[profiles.yenn.defaults]\nproviders = [\"x\"]", ["yenn"]) == "spec rejected: yenn.defaults is not allowed"


@pytest.mark.parametrize(
    ("entry", "field"),
    [
        ('required = "yes"', "required"),
        ("required = 1", "required"),
        ("description = 1", "description"),
        ("description = true", "description"),
        ("default = 5", "default"),
        ("default = [1]", "default"),
        ("composed = 1", "composed"),
        ("composed = false", "composed"),
    ],
)
def test_wrong_types(entry: str, field: str) -> None:
    assert reason(f"[profiles.p]\nK = {{ {entry} }}", ["p"]) == f"spec rejected: p.K.{field} has the wrong type"


def test_entry_not_table() -> None:
    assert reason('[profiles.p]\nK = "str"', ["p"]) == "spec rejected: p.K is not a table"
    assert reason("[profiles.p]\nK = 1", ["p"]) == "spec rejected: p.K is not a table"


def test_not_allowed_profile_even_if_absent() -> None:
    assert reason("", ["p"], allowed=set()) == "profile not allowed: p"
    assert reason("[profiles.p]\nA = {}", ["p"], allowed={"q"}) == "profile not allowed: p"


def test_not_allowed_checked_before_parsing() -> None:
    assert reason("[x", ["p"], allowed=set()) == "profile not allowed: p"
    assert reason("[x", ["p", "q"], allowed={"p"}) == "profile not allowed: q"


def test_not_allowed_wins_over_earlier_bad_profile() -> None:
    text = "[profiles.p]\nK = { generate = true }"
    assert reason(text, ["p", "q"], allowed={"p"}) == "profile not allowed: q"


def test_invalid_toml() -> None:
    assert reason("[x", ["p"]) == "spec rejected: invalid TOML"


def test_date_value_rejected_as_toml() -> None:
    r = reason("[profiles.p]\nK = { default = 1979-05-27 }", ["p"])
    assert r.startswith("spec rejected: ")


def test_unknown_profile() -> None:
    assert reason("", ["p"]) == "unknown profile: p"
    assert reason('[project]\nname = "x"', ["p"]) == "unknown profile: p"
    assert reason("[profiles.q]\nA = {}", ["p"]) == "unknown profile: p"
    assert reason("[profiles.p]\nA = {}", ["p", "q"], allowed={"p", "q"}) == "unknown profile: q"


def test_profile_not_a_table() -> None:
    assert reason("[profiles]\np = 1", ["p"]) == "spec rejected: profiles.p is not a table"
    assert reason('[profiles]\np = ["a"]', ["p"]) == "spec rejected: profiles.p is not a table"


def test_empty_profile() -> None:
    out, entries = canon("[profiles.p]", ["p"])
    assert entries == {"p": []}
    assert tomllib.loads(out)["profiles"] == {"p": {}}


def test_hostile_strings_round_trip() -> None:
    desc = 'q"uote \\ back\nnew é ☃ \t tab \x7f 😀'
    default = '"; [evil]\nx = 1 \\ \x7f 😀'
    key = "K"
    text = (
        "[profiles.p]\n"
        + tomllib_quote(key)
        + " = { description = "
        + tomllib_quote(desc)
        + ", default = "
        + tomllib_quote(default)
        + " }\n"
    )
    assert tomllib.loads(text)["profiles"]["p"][key]["description"] == desc
    out, entries = canon(text, ["p"])
    doc = tomllib.loads(out)
    assert doc["profiles"]["p"] == {key: {"description": desc, "default": default}}
    assert set(doc) == {"project", "providers", "defaults", "profiles"}
    assert entries["p"] == [Entry(key, description=desc, default=default)]
    assert out.isascii()


def test_hostile_profile_name_and_provider() -> None:
    prof = 'a"b'
    provider = 'bw://?folder=x"\n[evil]\ny = 1'
    out, _ = canon(f"[profiles.{tomllib_quote(prof)}]\nA = {{}}", [prof], provider=provider)
    doc = tomllib.loads(out)
    assert doc["providers"] == {"bw": provider}
    assert doc["profiles"] == {prof: {"A": {}}}
    assert "evil" not in doc


def tomllib_quote(s: str) -> str:
    out = []
    for ch in s:
        o = ord(ch)
        if ch in '"\\':
            out.append("\\" + ch)
        elif o < 0x20 or o == 0x7F or o > 0x7E:
            out.append(f"\\U{o:08x}")
        else:
            out.append(ch)
    return '"' + "".join(out) + '"'


def test_resolved_titles_contract_example() -> None:
    entries = [
        Entry("ACCESS_KEY_ID"),
        Entry("SECRET_ACCESS_KEY"),
        Entry("CLOUD", composed="${ACCESS_KEY_ID}..${SECRET_ACCESS_KEY}..${ACCESS_KEY_ID}"),
    ]
    assert resolved_titles("bw://?folder=infra/{profile}", "dijkstra-velero", entries) == [
        "infra/dijkstra-velero/ACCESS_KEY_ID",
        "infra/dijkstra-velero/SECRET_ACCESS_KEY",
        "CLOUD (composed of ACCESS_KEY_ID, SECRET_ACCESS_KEY)",
    ]


def test_resolved_titles_no_folder() -> None:
    entries = [Entry("A")]
    assert resolved_titles("bw://", "p", entries) == ["A"]
    assert resolved_titles("bw://?other=x", "p", entries) == ["A"]
    assert resolved_titles("bw://?folder=", "p", entries) == ["A"]


def test_resolved_titles_plain_folder_and_empty() -> None:
    assert resolved_titles("bw://?folder=infra", "p", [Entry("A")]) == ["infra/A"]
    assert resolved_titles(PROVIDER, "p", []) == []


def test_resolved_titles_composed_single() -> None:
    assert resolved_titles("bw://", "p", [Entry("C", composed="${X}")]) == ["C (composed of X)"]


@pytest.mark.parametrize("key", ["A B", "A/B", "A\nB", "1A", 'A"B', "A-B", "A.B", "é", "A\\B"])
def test_entry_key_must_be_identifier(key: str) -> None:
    with pytest.raises(SpecError):
        canon(f"[profiles.p]\n{tomllib_quote(key)} = {{}}", ["p"])


@pytest.mark.parametrize("key", ["A", "_", "_a1", "Zz_9", "a"])
def test_entry_key_identifier_accepted(key: str) -> None:
    _, entries = canon(f"[profiles.p]\n{key} = {{}}", ["p"])
    assert entries["p"] == [Entry(key)]


@pytest.mark.parametrize("composed", ["${BW_SESSION}", "x${OTHER}y", "${A}${NOPE}", "${a b}", "${}"])
def test_composed_must_reference_same_profile_entries(composed: str) -> None:
    text = f"[profiles.p]\nA = {{}}\nB = {{ composed = {tomllib_quote(composed)} }}"
    with pytest.raises(SpecError):
        canon(text, ["p"])


def test_composed_reference_to_other_profile_rejected() -> None:
    text = '[profiles.p]\nA = {}\n[profiles.q]\nB = { composed = "${A}" }'
    with pytest.raises(SpecError):
        canon(text, ["p", "q"])


@pytest.mark.parametrize("composed", ["${X", "a${X", "${A}${A"])
def test_composed_unterminated(composed: str) -> None:
    text = f"[profiles.p]\nA = {{}}\nX = {{}}\nB = {{ composed = {tomllib_quote(composed)} }}"
    with pytest.raises(SpecError):
        canon(text, ["p"])


def test_composed_valid_references_accepted() -> None:
    text = (
        "[profiles.p]\n"
        'A = {}\nB = {}\nC = { composed = "${A}..${B}..${A}" }\n'
        'D = { composed = "plain" }\nE = { composed = "${C}" }'
    )
    out, entries = canon(text, ["p"])
    assert tomllib.loads(out)["profiles"]["p"]["C"] == {"composed": "${A}..${B}..${A}"}
    assert [e.key for e in entries["p"]] == ["A", "B", "C", "D", "E"]


@pytest.mark.parametrize("value", ["a\x7fb", "😀", "l1\nl2", 'q"q', "b\\b", '\\"\n\x7f😀'])
def test_value_round_trip(value: str) -> None:
    text = f"[profiles.p]\nA = {{ description = {tomllib_quote(value)}, default = {tomllib_quote(value)} }}"
    out, entries = canon(text, ["p"])
    assert tomllib.loads(out)["profiles"]["p"]["A"] == {"description": value, "default": value}
    assert entries["p"] == [Entry("A", description=value, default=value)]


def test_spec_rejects_huge_int() -> None:
    with pytest.raises(SpecError):
        canon("[profiles.p]\nA = { default = " + "1" * 5000 + " }", ["p"])


def test_spec_rejects_deep_nesting() -> None:
    with pytest.raises(SpecError):
        canon("a = " + "[" * 100000, ["p"])
    with pytest.raises(SpecError):
        canon("[profiles.p]\nA = { default = " + "[" * 100000, ["p"])
