# Input: the secrets of one profile. $names: the secrets taken from it.
# Output: exactly those secrets, with lowercase names; fails if one is missing or empty.
with_entries(.key |= ascii_downcase)
| ($names - keys) as $missing
| if $missing != [] then error("missing in bws: \($missing)") end
| with_entries(select(.key | IN($names[])))
| [to_entries[] | select(.value == null or .value == "") | .key] as $empty
| if $empty != [] then error("empty in bws: \($empty)") end
