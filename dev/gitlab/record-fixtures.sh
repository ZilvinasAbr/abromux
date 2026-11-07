#!/usr/bin/env bash
# Save the local GitLab's API responses for the seeded merge requests as the
# GitLab plugin's offline test fixtures. Run after dev/gitlab/up.sh; rerun
# whenever seed.py changes or GitLab is upgraded, and review the diff.
set -euo pipefail
cd "$(dirname "$0")/../.."
URL=${GITLAB_URL:-http://localhost:8929}
TOKEN=${GITLAB_TOKEN:-glpat-abromux-dev-root-token}
OUT=crates/abromux-gitlab/tests/fixtures
P=$(printf %s alice/review-fixtures | jq -sRr @uri)
mkdir -p "$OUT"

get() { # path file
  curl -fsS -H "PRIVATE-TOKEN: $TOKEN" "$URL/api/v4/$1" | jq -S . >"$OUT/$2"
  echo "$OUT/$2"
}
get "version" version.json
get "user" user.json
get "merge_requests?scope=all&state=opened&per_page=100&with_labels_details=true" merge_requests.json
get "projects/$P/merge_requests?state=all&per_page=100" all_merge_requests.json
get "projects/$P/members/all?per_page=100" members.json
get "projects/$P/labels?per_page=100" labels.json
# !1's versions, each version's diffs, and the comparison of the two.
get "projects/$P/merge_requests/1/versions?per_page=100" mr1.versions.json
for v in $(jq -r '.[].id' "$OUT/mr1.versions.json"); do
  get "projects/$P/merge_requests/1/versions/$v" "mr1.versions.$v.json"
done
read -r new old < <(jq -r '[.[0].head_commit_sha, .[1].head_commit_sha] | @tsv' "$OUT/mr1.versions.json")
get "projects/$P/repository/compare?from=$old&to=$new&straight=true" compare.json
# Each of !1's commits on its own (the Commits tab opens one).
get "projects/$P/merge_requests/1/commits?per_page=100" mr1.commits.json >/dev/null
for sha in $(jq -r '.[].id' "$OUT/mr1.commits.json"); do
  get "projects/$P/repository/commits/$sha/diff?per_page=100" "commit.$sha.diff.json"
done
for iid in 1 2 3 4 5 6; do
  get "projects/$P/merge_requests/$iid/resource_state_events?per_page=100" "mr$iid.resource_state_events.json"
  get "projects/$P/merge_requests/$iid?include_diverged_commits_count=true" "mr$iid.json"
  if [ "$(jq -r .state "$OUT/mr$iid.json")" = merged ]; then
    get "projects/$P/pipelines?sha=$(jq -r .merge_commit_sha "$OUT/mr$iid.json")&per_page=1" "merge_pipelines.json"
  fi
  get "projects/$P/merge_requests/$iid/diffs?per_page=100" "mr$iid.diffs.json"
  get "projects/$P/merge_requests/$iid/discussions?per_page=100" "mr$iid.discussions.json"
  get "projects/$P/merge_requests/$iid/approvals" "mr$iid.approvals.json"
  get "projects/$P/merge_requests/$iid/award_emoji?per_page=100" "mr$iid.award_emoji.json"
  get "projects/$P/merge_requests/$iid/reviewers?per_page=100" "mr$iid.reviewers.json"
  get "projects/$P/merge_requests/$iid/closes_issues?per_page=100" "mr$iid.closes_issues.json"
  get "projects/$P/merge_requests/$iid/draft_notes?per_page=100" "mr$iid.draft_notes.json"
  get "projects/$P/merge_requests/$iid/pipelines?per_page=100" "mr$iid.pipelines.json"
  get "projects/$P/merge_requests/$iid/commits?per_page=100" "mr$iid.commits.json"
done
