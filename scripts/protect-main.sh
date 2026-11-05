#!/usr/bin/env bash
# Adds the "main" ruleset to an Accordion repo on GitHub, as every repo has:
# main can't be deleted or force-pushed, so its history and the commits
# other repos pin stay put.
#
# Usage: scripts/protect-main.sh accordion-<name>
#
# Run it once the repo is on GitHub, after its first push. It needs gh,
# signed in as an admin of the repo. A repo that has the ruleset already is
# left as it is.

set -euo pipefail

name="${1:-}"
if [[ ! "$name" =~ ^accordion-[a-z0-9-]+$|^accordion$ ]]; then
    echo "Usage: $0 accordion-<name>   (for example, accordion-drive)" >&2
    exit 1
fi
repo="accordion-suite/$name"

if gh api "repos/$repo/rulesets" --jq '.[].name' | grep -qx main; then
    echo "$repo already has the main ruleset."
    exit 0
fi

gh api -X POST "repos/$repo/rulesets" --input - --jq '"Added the \(.name) ruleset to '"$repo"': \(.enforcement)."' <<'JSON'
{
  "name": "main",
  "target": "branch",
  "enforcement": "active",
  "bypass_actors": [],
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "rules": [{ "type": "deletion" }, { "type": "non_fast_forward" }]
}
JSON
