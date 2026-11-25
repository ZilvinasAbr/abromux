#!/usr/bin/env bash
# Pipelines that finish, for the Reports tab and the report widgets: a
# GitLab Runner (shell executor) that only takes jobs tagged `reports`, so
# the fixtures' other pipelines stay pending as before, and a merge request
# whose pipeline writes a JUnit report (one failure) and a Code Quality one.
#
#   dev/gitlab/reports.sh          start the runner, make the merge request
#   dev/gitlab/reports.sh --down   remove both
set -euo pipefail
URL=http://localhost:8929
API=$URL/api/v4
H="PRIVATE-TOKEN: glpat-abromux-dev-root-token"
NAME=abromux-gitlab-runner
P=$(curl -fsS -H "$H" "$API/projects/alice%2Freview-fixtures" | jq -r .id)

if [ "${1:-}" = --down ]; then
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  for id in $(curl -fsS -H "$H" "$API/runners/all?tag_list=reports" | jq -r '.[].id'); do
    curl -fsS -X DELETE -H "$H" "$API/runners/$id"
  done
  for iid in $(curl -fsS -H "$H" "$API/projects/$P/merge_requests?source_branch=reports&state=all" | jq -r '.[].iid'); do
    curl -fsS -X DELETE -H "$H" "$API/projects/$P/merge_requests/$iid"
  done
  curl -fsS -X DELETE -H "$H" "$API/projects/$P/repository/branches/reports" || true
  echo removed; exit
fi

if ! docker ps --format '{{.Names}}' | grep -qx "$NAME"; then
  docker rm -f "$NAME" >/dev/null 2>&1 || true
  token=$(curl -fsS -X POST -H "$H" -H 'Content-Type: application/json' \
    -d "{\"runner_type\":\"project_type\",\"project_id\":$P,\"description\":\"reports\",\"tag_list\":[\"reports\"],\"run_untagged\":false}" \
    "$API/user/runners" | jq -r .token)
  docker run -d --name "$NAME" --add-host host.docker.internal:host-gateway gitlab/gitlab-runner:v18.5.0 >/dev/null
  docker exec "$NAME" gitlab-runner register --non-interactive --url http://host.docker.internal:8929 \
    --clone-url http://host.docker.internal:8929 --token "$token" --executor shell --name reports >/dev/null 2>&1
fi

ci=$(cat <<'YAML'
tests:
  tags: [reports]
  script:
    - |
      printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>' '<testsuites>' '<testsuite name="limiter" tests="4" failures="1" skipped="1" time="0.42">' '<testcase classname="limiter::tests" name="allows_under_limit" time="0.01"/>' '<testcase classname="limiter::tests" name="blocks_over_limit" time="0.02"><failure message="assertion failed">panicked at src/limiter.rs:17:9: assertion failed: allowed == false</failure></testcase>' '<testcase classname="limiter::tests" name="resets" time="0.30"/>' '<testcase classname="limiter::tests" name="slow_case" time="0"><skipped/></testcase>' '</testsuite>' '<testsuite name="config" tests="2" time="0.05">' '<testcase classname="config::tests" name="reads_env" time="0.03"/>' '<testcase classname="config::tests" name="defaults" time="0.02"/>' '</testsuite>' '</testsuites>' > junit.xml
    - |
      printf '%s' '[{"description":"Function allow has a Cognitive Complexity of 12 (exceeds 5 allowed).","check_name":"complexity","fingerprint":"r1","severity":"major","location":{"path":"src/limiter.rs","lines":{"begin":14}}},{"description":"Unused import HashSet.","check_name":"unused","fingerprint":"r2","severity":"minor","location":{"path":"src/limiter.rs","lines":{"begin":1}}}]' > gl-code-quality-report.json
    - exit 1
  artifacts:
    when: always
    reports:
      junit: junit.xml
      codequality: gl-code-quality-report.json
YAML
)
if ! curl -fsS -o /dev/null -H "$H" "$API/projects/$P/repository/branches/reports" 2>/dev/null; then
  curl -fsS -X POST -H "$H" -H 'Content-Type: application/json' -d '{"branch":"reports","ref":"main"}' "$API/projects/$P/repository/branches" >/dev/null
  curl -fsS -X POST -H "$H" -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg ci "$ci" '{branch:"reports",commit_message:"CI with reports",actions:[{action:"update",file_path:".gitlab-ci.yml",content:$ci}]}')" \
    "$API/projects/$P/repository/commits" >/dev/null
  curl -fsS -X POST -H "$H" -H 'Content-Type: application/json' \
    -d '{"source_branch":"reports","target_branch":"main","title":"Reports: tests and Code Quality"}' "$API/projects/$P/merge_requests" | jq -r '"made !\(.iid)"'
fi
echo "the pipeline takes ~30s; then the merge request has its reports"
