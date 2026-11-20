"""Secret and personal-data scanner: what must never reach an entry, a skill or the store."""

import re
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Literal

from . import data

Severity = Literal["hard", "soft", "pii"]

_MASK_LEN = 4


@dataclass(frozen=True)
class Finding:
    severity: Severity
    kind: str
    excerpt: str  # masked: the first 4 characters of the match, then an ellipsis


@dataclass(frozen=True)
class _Rule:
    severity: Severity
    kind: str
    pattern: re.Pattern[str]
    check: Callable[[str], bool] | None = None  # a further test of the matched text


def mask(text: str) -> str:
    """The first 4 characters and an ellipsis; never the whole value."""
    return text[:_MASK_LEN] + "…"


def _pesel_ok(digits: str) -> bool:
    """PESEL checksum: weights 1 3 7 9 repeated over the first ten digits, the last digit completes the sum to 10."""
    weights = (1, 3, 7, 9, 1, 3, 7, 9, 1, 3)
    total = sum(int(d) * w for d, w in zip(digits[:10], weights, strict=True))
    return (10 - total % 10) % 10 == int(digits[10])


def _mixed(value: str) -> bool:
    """Random-looking: at least one digit and one letter (a path, a placeholder or a plain name has no digit)."""
    return any(c.isdigit() for c in value) and any(c.isalpha() for c in value)


# the value of an assignment: what follows the first `:` or `=` of the matched text (the key has neither)
_ASSIGNED_VALUE = re.compile(r"""[:=][ \t]*['"]?([A-Za-z0-9_\-/+=.]+)\Z""")


def _credential_ok(matched: str) -> bool:
    """A value that is not a path (`/run/secrets/x`), a variable (`$TOKEN`), a template (`<token>`) or a placeholder."""
    m = _ASSIGNED_VALUE.search(matched)
    value = None if m is None else data.group(m, 1)
    return value is not None and value[0] not in "/$<" and _mixed(value)


def _bearer_ok(matched: str) -> bool:
    return _mixed(matched.split(None, 1)[1])


_RULES: tuple[_Rule, ...] = (
    _Rule(
        "hard",
        "private-key",
        re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY( BLOCK)?-----"),
    ),
    _Rule(
        "hard",
        "gitlab-token",
        re.compile(r"gl(?:pat|cbt|dt|rt|ptt|ft|oas)-[0-9A-Za-z_-]{20,}"),
    ),
    _Rule("hard", "github-token", re.compile(r"gh[pousr]_[A-Za-z0-9]{36,}")),
    _Rule("hard", "aws-access-key", re.compile(r"AKIA[0-9A-Z]{16}")),
    _Rule("hard", "slack-token", re.compile(r"xox[abprs]-[0-9A-Za-z-]{10,}")),
    _Rule(
        "hard",
        "jwt",
        re.compile(r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
    ),
    _Rule(
        "hard",
        "bearer-token",
        re.compile(r"Bearer\s+[A-Za-z0-9._~+/-]{20,}", re.IGNORECASE),
        _bearer_ok,
    ),
    _Rule(
        "hard",
        "sk-api-key",
        re.compile(r"(?<![A-Za-z0-9])sk-(?:ant-)?[A-Za-z0-9_-]{20,}"),
    ),
    # `access_token: …`, `"client_secret": "…"`, `export GITLAB_TOKEN=…`: the keyword anywhere in the name. No prefix is
    # matched (it cannot matter) and the suffix is capped and possessive, so that a long run of word characters costs no
    # more than a linear scan.
    _Rule(
        "hard",
        "credential-assignment",
        re.compile(
            r"""(?:api[_-]?key|secret|token|passphrase)[A-Za-z0-9_-]{0,40}+['"]?[ \t]*[:=][ \t]*['"]?[A-Za-z0-9_\-/+=.]{16,}""",
            re.IGNORECASE,
        ),
        _credential_ok,
    ),
    _Rule(
        "soft",
        "password-assignment",
        re.compile(
            r"\b(?:password|passwd|pwd|hasło|haslo)\b['\"]?[ \t]*[:=]", re.IGNORECASE
        ),
    ),
    _Rule("soft", "pgpassword", re.compile(r"PGPASSWORD=\S+")),
    # the scheme is bounded: an unbounded one is rescanned from every start of a long run of letters (quadratic)
    _Rule(
        "soft",
        "url-credentials",
        re.compile(r"[a-z][a-z0-9+.-]{0,30}://[^/\s:@]+:[^/\s@]+@", re.IGNORECASE),
    ),
    _Rule("soft", "sqlplus-credentials", re.compile(r"\b\w+/\w+@[\w.-]+:\d+/\w+")),
)
_PESEL = _Rule("pii", "pesel", re.compile(r"\b\d{11}\b"), _pesel_ok)

# markdown tables
_TABLE_SEPARATOR = re.compile(r"\s*\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)*\|?\s*")
_TABLE_SECRET_HEADER = re.compile(r"has[łl]o|password|passwd", re.IGNORECASE)


def _cells(row: str) -> list[str]:
    inner = row.strip().removeprefix("|").removesuffix("|")
    return [cell.strip() for cell in inner.split("|")]


def _table_findings(text: str) -> list[Finding]:
    """A table whose header has a password column and which has a data row."""
    lines = text.splitlines()
    found: list[Finding] = []
    for i in range(len(lines) - 2):
        header, separator, row = lines[i], lines[i + 1], lines[i + 2]
        if "|" not in header or "|" not in separator or "|" not in row:
            continue
        if (
            _TABLE_SEPARATOR.fullmatch(separator) is None
            or _TABLE_SEPARATOR.fullmatch(row) is not None
        ):
            continue
        for cell in _cells(header):
            if _TABLE_SECRET_HEADER.search(cell) is not None:
                found.append(Finding("soft", "password-table", mask(cell)))
                break
    return found


def _rule_findings(rule: _Rule, text: str) -> list[Finding]:
    found: list[Finding] = []
    for m in rule.pattern.finditer(text):
        matched = data.group(m)
        if matched is None or (rule.check is not None and not rule.check(matched)):
            continue
        found.append(Finding(rule.severity, rule.kind, mask(matched)))
    return found


def scan(text: str) -> list[Finding]:
    """Everything in `text` that looks like a secret (hard, soft) or personal data (pii), in rule order."""
    found: list[Finding] = []
    for rule in _RULES:
        found.extend(_rule_findings(rule, text))
    found.extend(_table_findings(text))
    found.extend(_rule_findings(_PESEL, text))
    return found


def has_hard(findings: Sequence[Finding]) -> bool:
    """True if any finding is a hard secret (a credential that must never be stored or submitted)."""
    return any(f.severity == "hard" for f in findings)
