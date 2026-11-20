# Input: the secrets of the bws project `hosts` (lowercase `<profile>/<key>` names).
# $p: the profile. $names: the secrets taken from it. $composed: the `composed` templates of every
# profile (secretspec.toml), by profile and key.
# Output: the secrets of the profile under their plain names, plus the composed ones in $names with
# their ${KEY} inputs substituted; fails if an input is missing or empty.
with_entries(select(.key | startswith($p + "/")) | .key |= ltrimstr($p + "/")) as $vals
| (
  ($composed[$p] // {})
  | to_entries
  | map(select(.key | ascii_downcase | IN($names[])))
  | map(
    .value as $template
    | ([$template | match("\\$\\{(?<k>[A-Za-z0-9_]+)\\}"; "g").captures[0].string | ascii_downcase | select(($vals[.] // "") == "")] | unique) as $bad
    | if $bad != [] then error("\($p)/\(.key) needs missing or empty \($bad)")
      else {key: (.key | ascii_downcase), value: ($template | gsub("\\$\\{(?<k>[A-Za-z0-9_]+)\\}"; $vals[.k | ascii_downcase]))}
      end
  )
  | from_entries
) as $rendered
| $vals + $rendered
