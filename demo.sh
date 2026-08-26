#!/bin/sh
# test -> fail -> capture -> capsule -> edit -> replay -> analyze -> result
set -eu
cd "$(dirname "$0")"

TRACEDNA="cargo run --release --quiet --"
step() { printf '\n== %s\n\n' "$*"; }

step "1. the subject: what runs, and under which conditions"
cat subject.capsule

step "2. capture: look for a seed that breaks it, then freeze that run"
$TRACEDNA capture subject.capsule

step "3. replay: same capsule, same failure, as often as you like"
$TRACEDNA replay failure.capsule || true

step "4. edit one field: env.CURRENCY BRL -> USD"
sed -i.bak 's/^env\.CURRENCY=BRL$/env.CURRENCY=USD/' failure.capsule
$TRACEDNA replay failure.capsule || true
mv failure.capsule.bak failure.capsule

step "5. analyze: remove one method at a time, replay under the frozen conditions"
$TRACEDNA analyze failure.capsule

step "6. the finished artifact"
cat failure.capsule
