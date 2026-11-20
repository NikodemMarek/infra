"""Every fixture is obviously fake and assembled from pieces, so the repo itself does not trip a scanner."""

import pytest
from knowledge import secrets
from knowledge.secrets import Finding

FAKE = "fake" * 5  # 20 characters of [a-z]
MIXED = "fake1234" * 3  # 24 characters, letters and digits: looks random


def kinds(text: str) -> list[str]:
    return [f.kind for f in secrets.scan(text)]


# hard


def test_private_key():
    assert kinds("-----BEGIN " + "RSA PRIVATE KEY" + "-----\nAAAA") == ["private-key"]
    assert kinds("-----BEGIN " + "PRIVATE KEY" + "-----") == ["private-key"]
    assert kinds("-----BEGIN " + "PGP PRIVATE KEY BLOCK" + "-----") == ["private-key"]
    assert kinds("-----BEGIN " + "OPENSSH PRIVATE KEY" + "-----") == ["private-key"]
    assert kinds("-----BEGIN " + "PUBLIC KEY" + "-----") == []
    assert kinds("-----BEGIN " + "PGP PUBLIC KEY BLOCK" + "-----") == []


def test_gitlab_token():
    assert kinds("glpat" + "-" + FAKE) == ["gitlab-token"]
    assert kinds("glpat" + "-" + "short") == []


@pytest.mark.parametrize("prefix", ["glcbt", "gldt", "glrt", "glptt", "glft", "gloas"])
def test_other_gitlab_token_kinds(prefix: str):
    assert kinds(prefix + "-" + FAKE) == ["gitlab-token"]
    assert kinds(prefix + "-" + "short") == []
    assert kinds("glxx" + "-" + FAKE) == []


def test_api_key_with_the_sk_prefix():
    assert kinds("sk" + "-" + FAKE) == ["sk-api-key"]
    assert kinds("sk" + "-ant-" + "api03-" + FAKE) == ["sk-api-key"]
    assert kinds("OPENAI " + "sk" + "-proj-" + FAKE) == ["sk-api-key"]
    assert kinds("sk" + "-" + "short") == []
    assert kinds("the task-management-system-config-file") == []  # `sk-` inside a word
    assert kinds("disk-usage-report-for-the-whole-cluster") == []


def test_github_token():
    for prefix in ("ghp", "gho", "ghu", "ghs", "ghr"):
        assert kinds(prefix + "_" + "f" * 36) == ["github-token"]
    assert kinds("ghp" + "_" + "f" * 10) == []
    assert kinds("ghx" + "_" + "f" * 36) == []


def test_aws_access_key():
    assert kinds("AKIA" + "FAKE" * 4) == ["aws-access-key"]
    assert kinds("AKIA" + "FAKE" * 3) == []
    assert kinds("akia" + "fake" * 4) == []


def test_slack_token():
    assert kinds("xoxb" + "-" + "0000000000-fake") == ["slack-token"]
    assert kinds("xoxz" + "-" + "0000000000-fake") == []
    assert kinds("xoxb" + "-" + "short") == []


def test_jwt():
    jwt = ".".join(["eyJ" + "fakefakefake", "fakefakefake", "fakefakefake"])
    assert kinds("Authorization: Bearer " + jwt) == ["jwt"]
    assert kinds("eyJ" + "fakefakefake" + ".fake") == []


def test_bearer_token():
    assert kinds("Authorization: Bearer " + MIXED) == ["bearer-token"]
    assert kinds("curl -H 'Authorization: bearer " + MIXED + "'") == ["bearer-token"]
    assert kinds("Bearer " + "abc123." + "def456/ghi789+jkl012~mno345") == [
        "bearer-token"
    ]
    assert kinds("Bearer " + "short1") == []
    assert kinds("Bearer " + FAKE) == []  # no digit: not random looking
    assert kinds("Bearer " + "1234567890" * 3) == []  # no letter
    assert kinds("a Bearer authentication scheme") == []


def test_credential_assignment():
    for name in (
        "api_key",
        "api-key",
        "apikey",
        "API_KEY",
        "secret",
        "token",
        "passphrase",
    ):
        assert kinds(name + " = " + MIXED) == ["credential-assignment"], name
    assert kinds("token='" + MIXED + "'") == ["credential-assignment"]
    assert kinds("secret: " + MIXED) == ["credential-assignment"]
    assert kinds("token: short") == []
    assert kinds("the token " + MIXED) == []  # no : or =
    assert kinds("token: " + FAKE) == []  # no digit
    assert kinds("token: " + "1234567890" * 3) == []  # no letter
    assert kinds("token: " + MIXED[:15]) == []  # 15 characters
    assert kinds("token: " + MIXED[:16]) == ["credential-assignment"]


@pytest.mark.parametrize(
    "text",
    [
        "access_token: " + MIXED,
        '"access_token": "' + MIXED + '"',
        "'access_token': '" + MIXED + "'",
        "client_secret=" + MIXED,
        "SECRET_KEY=" + MIXED,
        "OPENAI_API_KEY=" + MIXED,
        "export GITLAB_TOKEN=" + MIXED,
        "GITLAB-TOKEN: " + MIXED,
        "mytoken: " + MIXED,
        "tokenizer_secret_value = " + MIXED,
        "--api-key=" + MIXED,
        "DB_PASSPHRASE: " + MIXED,
        "token=" + "abcd1234" * 2 + "==",
        "token: " + "abcd1234" * 2 + "-/+._",
        '{"api_key":' + '"' + MIXED + '"}',
        "x = 1\nsecret_token  :\t" + MIXED + "\nnext line",
    ],
)
def test_credential_assignment_in_the_forms_secrets_come_in(text: str):
    assert kinds(text) == ["credential-assignment"]


@pytest.mark.parametrize(
    "text",
    [
        "token: /var/run/secrets/kubernetes.io/serviceaccount/token",
        "secret: /run/secrets/db_password",
        "secretFile: /run/secrets/db_password_2",
        "token_path = /var/run/secrets/kubernetes.io/serviceaccount/token",
        "api_key: YOUR_API_KEY_PLACEHOLDER",
        "api_key: " + "YOUR_API_KEY_HERE" + "_" + "x" * 8,
        "secret: postgres-credentials-secret",
        "token: $" + "GITLAB_TOKEN_FROM_THE_ENVIRONMENT",
        "token: <" + "your-token-goes-here-2024>",
        "token: ${" + "GITLAB_TOKEN}",
        "- name: db-secret\n  secretName: postgres-credentials-secret",
        "docker secret create db_password_v2 /run/secrets/db_password_v2",
        "kubectl get secret postgres-credentials -o jsonpath='{.data.token}'",
        "token: " + "/" + MIXED,
    ],
)
def test_credential_assignment_ignores_paths_variables_and_placeholders(text: str):
    assert kinds(text) == []


def test_credential_assignment_is_linear_in_the_text():
    """Long runs of word characters must not make the rule slow (it is run on every submission)."""
    rule = next(r for r in secrets._RULES if r.kind == "credential-assignment")
    for text in (
        "token" * 20000,
        "token_" * 20000,
        "a-" * 50000,
        "x" * 100000 + "token",
        "secret=" * 10000,
    ):
        assert secrets._rule_findings(rule, text) == [], text[:12]


# soft


def test_password_assignment():
    for word in ("password", "Password", "passwd", "pwd", "Hasło", "haslo", "HASŁO"):
        assert kinds(word + ": hunter2") == ["password-assignment"], word
    assert kinds("password = x") == ["password-assignment"]
    assert kinds("a password manager") == []
    assert kinds("Password reset flow") == []


def test_pgpassword():
    assert kinds("PGPASSWORD=" + "fake-pass psql -h db") == ["pgpassword"]
    assert kinds("PGPASSWORD=") == []
    assert kinds("export PGPASSFILE=/x") == []


def test_url_userinfo():
    assert kinds("postgres://" + "user:fakepass" + "@db.example.com/app") == [
        "url-credentials"
    ]
    assert kinds("https://" + "user:fake@" + "example.com") == ["url-credentials"]
    assert kinds("https://example.com:8080/path") == []
    assert kinds("git@example.com:group/repo.git") == []
    assert kinds("ssh://git@example.com/repo.git") == []
    assert kinds("see https://example.com/a and mail me@example.com") == []


def test_url_userinfo_is_linear_in_the_text():
    """A long run of scheme characters must not be rescanned from each of its starts (a quadratic scan)."""
    rule = next(r for r in secrets._RULES if r.kind == "url-credentials")
    for text in (
        "a" * 100000,
        "a-" * 50000,
        "a.b+" * 25000,
        "a" * 100000 + "://",
        "a://" + "b:" * 50000,
        "a://b" * 20000,
    ):
        assert secrets._rule_findings(rule, text) == [], text[:12]
    assert kinds("a" * 100000 + "://" + "user:fake@" + "example.com") == [
        "url-credentials"
    ]  # the scheme is cut, still found
    assert kinds("x" * 31 + "://" + "user:fake@" + "example.com") == ["url-credentials"]


def test_sqlplus_style():
    assert kinds("sqlplus " + "scott/" + "fake@" + "db.example.com:1521/ORCL") == [
        "sqlplus-credentials"
    ]
    assert kinds("scott/fake@db.example.com") == []  # no port and service
    assert kinds("path/to@host:notaport/x") == []


def test_markdown_table_with_password_column():
    table = "| Użytkownik | Hasło |\n|---|---|\n| sa | not-a-secret |\n"
    found = secrets.scan(table)
    assert [(f.severity, f.kind) for f in found] == [("soft", "password-table")]
    assert found[0].excerpt == "Hasł…"
    assert kinds("Host | Password\n--- | :---:\nweb1 | x") == ["password-table"]
    assert kinds("| passwd |\n|--|\n| x |") == ["password-table"]
    # a header without a data row is not a finding
    assert kinds("| User | Hasło |\n|---|---|\n") == []
    assert kinds("| User | Hasło |\n|---|---|\n\nlater text | with pipe |") == []


def test_markdown_table_without_password_column():
    ips = "| Host | IP |\n|---|---|\n| web1 | 192.0.2.10 |\n| web2 | 192.0.2.11 |\n"
    assert secrets.scan(ips) == []
    assert kinds("a | b\nnot a separator\nc | d") == []
    assert kinds("Hasło\n---\nx") == []  # no pipes: a heading, not a table


# pii


def test_pesel_with_valid_checksum():
    found = secrets.scan("PESEL " + "4405140" + "1359")
    assert found == [Finding("pii", "pesel", "4405…")]
    assert kinds("(" + "44051401359" + ")") == ["pesel"]


def test_pesel_with_bad_checksum():
    assert kinds("44051401358") == []
    assert kinds("00000000001") == []


def test_pesel_needs_exactly_eleven_digits():
    assert kinds("440514013590") == []
    assert kinds("4405140135") == []
    assert (
        kinds("a" + "44051401359") == []
    )  # \b: a letter glued to the digits is not a boundary


# masking and has_hard


def test_excerpt_is_masked():
    secret = "glpat" + "-" + FAKE
    (found,) = secrets.scan("token " + secret)
    assert found == Finding("hard", "gitlab-token", "glpa…")
    assert secret not in found.excerpt

    (found,) = secrets.scan("PGPASSWORD=" + "hunter2")
    assert found.excerpt == "PGPA…"
    assert secrets.mask("abc") == "abc…"


def test_has_hard():
    hard = secrets.scan("AKIA" + "FAKE" * 4)
    soft = secrets.scan("password: x")
    pii = secrets.scan("44051401359")
    assert secrets.has_hard(hard)
    assert not secrets.has_hard(soft)
    assert not secrets.has_hard(pii)
    assert secrets.has_hard(soft + hard)
    assert not secrets.has_hard([])


def test_clean_text_and_several_findings():
    assert secrets.scan("") == []
    assert (
        secrets.scan(
            "Use `jj new` before spawning a coder; see src/knowledge/cli.py:42."
        )
        == []
    )
    text = "PGPASSWORD=" + "fake psql\n" + "AKIA" + "FAKE" * 4 + "\n44051401359"
    assert kinds(text) == ["aws-access-key", "pgpassword", "pesel"]


def test_json_style_keys_are_assignments() -> None:
    kinds = {f.kind for f in secrets.scan('{"' + "token" + '": "' + MIXED + '"}')}
    assert "credential-assignment" in kinds
    assert "password-assignment" in {
        f.kind for f in secrets.scan('{"pass' + 'word": "x"}')
    }


def test_password_header_above_a_separator_is_not_an_assignment() -> None:
    kinds = {f.kind for f in secrets.scan("Password\n:---\n")}
    assert "password-assignment" not in kinds
