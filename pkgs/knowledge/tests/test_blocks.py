from knowledge.blocks import Block, count_fences, parse_blocks

FULL = """\
```knowledge
title: jj absorb needs --into when trunk() has no remote bookmark
kind: gotcha
scope: topic/jj
evidence: dotfiles 2026-10-06: absorb rewrote published commits
---
Rule, why, example.
```"""


def test_a_full_block():
    assert parse_blocks(FULL) == [
        Block(
            title="jj absorb needs --into when trunk() has no remote bookmark",
            body="Rule, why, example.",
            kind="gotcha",
            scope="topic/jj",
            evidence="dotfiles 2026-10-06: absorb rewrote published commits",
        )
    ]


def test_only_the_title_is_required():
    got = parse_blocks("```knowledge\ntitle: t\n---\nbody\n```")
    assert got == [Block("t", "body", None, None, None)]


def test_several_blocks_between_prose():
    text = f"Done. Two things I learned:\n\n{FULL}\n\nAnd also:\n\n```knowledge\ntitle: second\n---\nline 1\n\nline 2\n```\nThat is all.\n"
    got = parse_blocks(text)
    assert [b.title for b in got] == ["jj absorb needs --into when trunk() has no remote bookmark", "second"]
    assert got[1].body == "line 1\n\nline 2"


def test_a_block_without_a_title_is_skipped():
    text = "```knowledge\nkind: gotcha\n---\nbody\n```\n```knowledge\ntitle:   \n---\nbody\n```\n" + FULL
    got = parse_blocks(text)
    assert len(got) == 1
    assert got[0].title.startswith("jj absorb")


def test_a_block_without_a_separator_is_skipped():
    assert parse_blocks("```knowledge\ntitle: t\nthe body, but no separator\n```") == []


def test_a_block_without_a_body_is_skipped():
    assert parse_blocks("```knowledge\ntitle: t\n---\n  \n\n```") == []


def test_other_fences_are_ignored():
    text = (
        "```python\ntitle: t\n---\nbody\n```\n"
        "```knowledgebase\ntitle: t\n---\nbody\n```\n"
        "``` knowledge\ntitle: t\n---\nbody\n```\n"
        "    ```knowledge\n    title: t\n    ---\n    body\n    ```\n"
        "~~~knowledge\ntitle: t\n---\nbody\n~~~\n"
    )
    assert parse_blocks(text) == []


def test_the_title_is_one_line_and_the_header_keeps_the_last_value_of_a_key():
    got = parse_blocks("```knowledge\ntitle:  a   b \ntitle: c   d\nkind: \nunknown: x\nnot a header line\n---\nbody\n```")
    assert got == [Block("c d", "body", None, None, None)]


def test_the_body_may_hold_a_separator_and_the_fence_may_have_trailing_spaces():
    got = parse_blocks("```knowledge  \ntitle: t\n---\nbefore\n---\nafter\n```  \n")
    assert got == [Block("t", "before\n---\nafter", None, None, None)]


def test_an_indented_block_is_dedented():
    text = "- one thing:\n  ```knowledge\n  title: t\n  kind: gotcha\n  ---\n  line 1\n    indented\n  ```\n"
    assert parse_blocks(text) == [Block("t", "line 1\n  indented", "gotcha", None, None)]


def test_the_fences_may_be_indented_by_up_to_three_spaces_or_tabs():
    assert parse_blocks("   ```knowledge\ntitle: t\n---\nbody\n ```") == [Block("t", "body", None, None, None)]
    assert parse_blocks("\t```knowledge\n\ttitle: t\n\t---\n\tbody\n\t```") == [Block("t", "body", None, None, None)]
    assert parse_blocks("    ```knowledge\n    title: t\n    ---\n    body\n    ```") == []  # four spaces: code


def test_a_block_with_four_backticks_may_hold_a_fenced_example():
    text = "````knowledge\ntitle: t\n---\nUse:\n```bash\nls -l\n```\nthen stop.\n````\n"
    assert parse_blocks(text) == [Block("t", "Use:\n```bash\nls -l\n```\nthen stop.", None, None, None)]


def test_the_fence_closes_with_as_many_backticks_as_it_opened():
    assert parse_blocks("````knowledge\ntitle: t\n---\nbody\n```") == []
    assert parse_blocks("```knowledge\ntitle: t\n---\nbody\n````") == []
    assert parse_blocks("`````knowledge\ntitle: t\n---\nbody\n`````") == []


def test_crlf_text():
    got = parse_blocks("```knowledge\r\ntitle: t\r\n---\r\nbody\r\n```\r\n")
    assert got == [Block("t", "body", None, None, None)]


def test_no_blocks():
    assert parse_blocks("") == []
    assert parse_blocks("nothing to learn here") == []


def test_count_fences_counts_the_invalid_blocks_too():
    text = "```knowledge\ntitle: t\n---\nbody\n```\n```knowledge\nno title\n```\n```bash\nls\n```"
    assert count_fences(text) == 2
    assert len(parse_blocks(text)) == 1
    assert count_fences("") == 0
