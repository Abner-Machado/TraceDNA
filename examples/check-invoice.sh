#!/bin/sh
# Invoice exits 0 even when the invoice is wrong, so its exit code says nothing.
# This reads what it printed instead.
# 0 = invoice is fine, 1 = a negative total, 2 = output too broken to judge.
out=$(cat)
if [ "$(printf '%s\n' "$out" | grep -c 'total=')" -ne 4 ]; then
    echo "expected 4 invoice lines" >&2
    exit 2
fi
if printf '%s\n' "$out" | grep -q 'total=-'; then
    echo "negative invoice total" >&2
    exit 1
fi
