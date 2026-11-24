function __jw_repos # a glob matching nothing is an error in a command (only `set` is exempt)
    set -l d ~/projects/*/.jj
    path dirname $d | path basename
end

function __jw_workspaces # directory names of this repo's workspaces, from the main repo or from one of them
    set -l root (jj --ignore-working-copy workspace root 2>/dev/null); or return
    set -l d (string replace -r '\.agents/[^/]+$' '' -- $root).agents/*/.jj
    set -q d[1]; or return
    path dirname $d | path basename
end

function __jw_shpool_sessions # first column of `shpool list`, after the header
    shpool --no-daemonize list 2>/dev/null | string match -rg '^(\S+)' | tail -n +2
end

complete -c jw -f
complete -c jw -n __fish_use_subcommand -a 'new n cd c rm clone prune reap poll info pick agent a resume r attach msg session done mate'
complete -c jw -n '__fish_seen_subcommand_from prune' -l yes -d "don't ask; answer yes to every question"
complete -c jw -n '__fish_seen_subcommand_from new n' -s r -l revision -r -d 'revision to start from (default: trunk())'
complete -c jw -n '__fish_seen_subcommand_from new n' -a '(__jw_repos)'
complete -c jw -n '__fish_seen_subcommand_from agent a' -s r -l revision -r -d 'revision to start from (default: trunk())'
complete -c jw -n '__fish_seen_subcommand_from agent a' -s d -l detach -d 'start the session in the background'
complete -c jw -n '__fish_seen_subcommand_from agent a' -l reports-to -r -d 'the session this one reports to'
complete -c jw -n '__fish_seen_subcommand_from agent a' -a '(__jw_repos)'
complete -c jw -n '__fish_seen_subcommand_from resume r' -s d -l detach -d 'start the session in the background'
complete -c jw -n '__fish_seen_subcommand_from resume r' -l session -r -d 'resume the last session of the agent session NAME'
complete -c jw -n '__fish_seen_subcommand_from resume r' -a '(__jw_repos)'
complete -c jw -n '__fish_seen_subcommand_from cd c rm' -a '(__jw_workspaces)'
complete -c jw -n '__fish_seen_subcommand_from attach' -a '(__jw_shpool_sessions)'
complete -c jw -n '__fish_seen_subcommand_from session; and not __fish_seen_subcommand_from get set status worktree list' -a 'get set status worktree list'
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from status; and not __fish_seen_subcommand_from busy idle waiting ended' -a 'busy idle waiting ended'
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from status' -l waiting-for -r -d 'what the session waits for (with waiting)'
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from status' -l cache-expires-at -r -d 'when the prompt cache goes cold (epoch seconds)'
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from list' -l reports-to -r -d 'only the sessions that report to the session NAME'
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from get set' -s s -l session -r -d 'session name (default: $AGENT_SESSION)'
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from set' -l kind -r -d 'what started the session'
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from set' -l title -r
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from set' -l description -r
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from set' -l workspace -r -F
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from set' -l url -r -d 'https link to the work item'
complete -c jw -n '__fish_seen_subcommand_from session; and __fish_seen_subcommand_from set' -l mr -r -d 'https url of a merge request to add'
