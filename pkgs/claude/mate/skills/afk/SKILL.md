---
name: afk
description: Away mode for the first mate session: the user is away, so decide routine questions alone and queue the rest until they return. Only the user invokes it.
disable-model-invocation: true
argument-hint: "[standing instructions for while I'm away]"
---

# Away mode

Only for the first mate (role "first mate" in CLAUDE.md); anywhere else, say so and stop.

1. Write the user's instructions (the arguments, verbatim; empty if none) to `${XDG_STATE_HOME:-$HOME/.local/state}/jw/$AGENT_SESSION.away`, creating the directory. Running it again replaces them.
2. Reply in one line that away mode is on, then follow "Away mode" in your CLAUDE.md until the user returns.
