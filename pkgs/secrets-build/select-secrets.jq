# Input: the secrets of the bws project `hosts`, as {"<profile>/<KEY>": value}.
# $p: the profile. $names: the secrets taken from it.
# Output: exactly those secrets of the profile, with lowercase names; fails if one is missing or empty.
with_entries(select(.key | startswith($p + "/")) | .key |= ltrimstr($p + "/"))
| with_entries(.key |= ascii_downcase)
| ($names - keys) as $missing
| if $missing != [] then error("missing in bws: \($missing)") end
| with_entries(select(.key | IN($names[])))
| [to_entries[] | select(.value == null or .value == "") | .key] as $empty
| if $empty != [] then error("empty in bws: \($empty)") end
