#!/bin/sh
# stop-verify.sh — keep the agent working until the build passes.

INPUT=$(cat)

# Gate only genuine turn ends, not the observe-only session-end fire.
REASON=$(echo "$INPUT" | grep -o '"reason":"[^"]*"' | sed 's/"reason":"//;s/"$//')
if [ "$REASON" != "end_turn" ]; then
  exit 0
fi

if cargo build --quiet >/dev/null 2>&1; then
  # Build is green: allow the stop.
  exit 0
fi

# Build is red: keep the agent working, with the failure as feedback.
echo '{"decision":"block","reason":"cargo build failed; fix the errors before finishing."}'
