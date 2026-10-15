# Claude Code permission rules (https://code.claude.com/docs/en/permissions),
# imported by settings.nix as `permissions`. `@memoryDir@` is substituted at
# build time.
{
  allow = [
    "Bash(jj log:*)"
    "Bash(jj status:*)"
    "Bash(jj show:*)"
    "Bash(jj diff:*)"
    "Bash(jj new:*)"
    "Bash(jj squash:*)"
    "Bash(jj split:*)"
    "Bash(jj edit:*)"
    "Bash(jj describe:*)"
    "Bash(jj commit:*)"
    "Bash(jj rebase:*)"
    "Bash(jj upload:*)"
    "Bash(jw agent -d:*)"

    # Read-only glab. Not `glab api` (can POST/PUT), `glab ci view` or
    # `glab issue board view` (interactive TUIs that can run/retry jobs).
    "Bash(glab mr list:*)"
    "Bash(glab mr view:*)"
    "Bash(glab mr diff:*)"
    "Bash(glab mr approvers:*)"
    "Bash(glab mr issues:*)"
    "Bash(glab mr note list:*)"
    "Bash(glab issue list:*)"
    "Bash(glab issue view:*)"
    "Bash(glab ci list:*)"
    "Bash(glab ci get:*)"
    "Bash(glab ci status:*)"
    "Bash(glab ci trace:*)"
    "Bash(glab ci lint:*)"
    "Bash(glab ci config compile:*)"
    "Bash(glab repo view:*)"
    "Bash(glab repo list:*)"
    "Bash(glab repo contributors:*)"
    "Bash(glab release list:*)"
    "Bash(glab release view:*)"
    "Bash(glab label list:*)"
    "Bash(glab label get:*)"

    # Queueing, recall and read-only status of the knowledge curator.
    "Bash(knowledge submit:*)"
    "Bash(knowledge status:*)"
    "Bash(knowledge ask:*)"

    "Bash(mvn test:*)"
    "Bash(mvn compile:*)"
  ];
  deny = [];
  ask = [];
  additionalDirectories = ["@memoryDir@" "~/projects"];
}
