#!/usr/bin/env bash
# Start the local GitLab, wait until it answers, make a known admin token and
# seed the review fixtures. Safe to rerun: the token is reset and seed.py
# recreates its projects.
#   dev/gitlab/up.sh            start + seed
#   dev/gitlab/up.sh --no-seed  start only
#   dev/gitlab/up.sh --down     stop (data kept; `docker compose ... down -v` wipes it)
set -euo pipefail
cd "$(dirname "$0")"
COMPOSE=(docker compose -f compose.yml)
CONTAINER=abromux-gitlab-gitlab-1
URL=http://localhost:8929
TOKEN=glpat-abromux-dev-root-token
BOB_TOKEN=glpat-abromux-dev-bob-token

if [ "${1:-}" = --down ]; then "${COMPOSE[@]}" stop; exit; fi

"${COMPOSE[@]}" up -d
printf 'waiting for GitLab at %s ' "$URL"
for _ in $(seq 1 180); do
  if curl -fsS -o /dev/null "$URL/users/sign_in" 2>/dev/null; then echo ' up'; break; fi
  printf .; sleep 5
done
curl -fsS -o /dev/null "$URL/users/sign_in" || { echo ' timed out'; exit 1; }

# A fixed admin token (api + sudo, so seed.py can act as alice and bob).
docker exec "$CONTAINER" gitlab-rails runner "
  u = User.find_by_username('root')
  u.personal_access_tokens.where(name: 'abromux-dev').delete_all
  t = u.personal_access_tokens.build(name: 'abromux-dev', scopes: [:api, :sudo], expires_at: 300.days.from_now)
  t.set_token('$TOKEN'); t.save!
" >/dev/null
echo "root token: $TOKEN"

[ "${1:-}" = --no-seed ] || python3 -I seed.py --url "$URL" --token "$TOKEN"

# bob, the seeded reviewer: the token the app uses (dev/gitlab/app.sh); api
# scope, to comment, resolve and approve.
docker exec "$CONTAINER" gitlab-rails runner "
  u = User.find_by_username('bob')
  u.personal_access_tokens.where(name: 'abromux-dev').delete_all
  t = u.personal_access_tokens.build(name: 'abromux-dev', scopes: [:api], expires_at: 300.days.from_now)
  t.set_token('$BOB_TOKEN'); t.save!
  # Dark mode, like the tab: GitLab's pages are the reference for it.
  u.update!(color_mode_id: 2)
" >/dev/null
echo "bob token:  $BOB_TOKEN"
