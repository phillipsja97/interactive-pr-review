#!/bin/sh
# Stand-in for the GitHub CLI in prr's tests. Answers from files next to this script and logs every call.
D="$(dirname "$0")"
echo "$*" >> "$D/calls.log"
case "$*" in
  "repo view"*) echo "https://github.com/o/r" ;;
  *"/reviews --input -")
    if [ -f "$D/fail" ]; then cat "$D/fail" >&2; exit 1; fi
    cat > "$D/payload.json"
    echo '{"id":1,"html_url":"https://github.com/o/r/pull/7#pullrequestreview-1","state":"COMMENTED"}' ;;
  "api"*" user") printf '{"login":"%s"}\n' "$(cat "$D/viewer")" ;;
  "api"*"/pulls/"*) cat "$D/pr.json" ;;
  *) echo "unexpected gh call: $*" >&2; exit 2 ;;
esac
