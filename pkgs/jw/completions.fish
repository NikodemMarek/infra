function __jw_repos  # a glob matching nothing is an error in a command (only `set` is exempt)
    set -l d ~/projects/*/.jj
    path dirname $d | path basename
end

function __jw_workspaces  # directory names of this repo's workspaces, from the main repo or from one of them
    set -l root (jj --ignore-working-copy workspace root 2>/dev/null); or return
    set -l d (string replace -r '\.agents/[^/]+$' '' -- $root).agents/*/.jj
    set -q d[1]; or return
    path dirname $d | path basename
end

function __jw_shpool_sessions  # first column of `shpool list`, after the header
    shpool --no-daemonize list 2>/dev/null | string match -rg '^(\S+)' | tail -n +2
end

complete -c jw -f
complete -c jw -n __fish_use_subcommand -a 'new n cd c rm clone prune reap info ls pick agent a attach at msg'
complete -c jw -n '__fish_seen_subcommand_from new n' -s r -l revision -r -d 'revision to start from (default: trunk())'
complete -c jw -n '__fish_seen_subcommand_from new n' -a '(__jw_repos)'
complete -c jw -n '__fish_seen_subcommand_from agent a' -s r -l revision -r -d 'revision to start from (default: trunk())'
complete -c jw -n '__fish_seen_subcommand_from agent a' -s d -l detach -d 'start the session in the background'
complete -c jw -n '__fish_seen_subcommand_from agent a' -a '(__jw_repos)'
complete -c jw -n '__fish_seen_subcommand_from cd c rm' -a '(__jw_workspaces)'
complete -c jw -n '__fish_seen_subcommand_from attach at' -a '(__jw_shpool_sessions)'
