#!/usr/bin/env bash
set -euo pipefail

# Usage: generate-changelog.sh [base-ref]
# Generate changelog from conventional commits since the previous tag.
# If base-ref is given, use it instead of auto-detecting the previous tag.

BASE_REF="${1:-$(git tag --sort=-creatordate | head -n 1)}"

if [ -z "$BASE_REF" ]; then
  echo "No previous tag found. Listing all commits."
  RANGE="HEAD"
else
  RANGE="$BASE_REF..HEAD"
fi

for TYPE in feat fix docs refactor test chore ci; do
  LOG=$(git log "$RANGE" --pretty=format:"- %s (%h)" --grep="^$TYPE" || true)
  if [ -n "$LOG" ]; then
    echo "### $TYPE"
    echo ""
    echo "$LOG"
    echo ""
  fi
done

REPO_URL=$(git remote get-url origin | sed 's/\.git$//' | sed 's|git@github.com:|https://github.com/|')
if [ -n "$BASE_REF" ]; then
  echo "**Full Changelog**: $REPO_URL/compare/$BASE_REF...HEAD"
fi
