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

complete -c jw -f
complete -c jw -n __fish_use_subcommand -a 'new n cd c rm clone prune info ls'
complete -c jw -n '__fish_seen_subcommand_from new n' -s r -l revision -r -d 'revision to start from (default: trunk())'
complete -c jw -n '__fish_seen_subcommand_from new n' -a '(__jw_repos)'
complete -c jw -n '__fish_seen_subcommand_from cd c rm' -a '(__jw_workspaces)'
