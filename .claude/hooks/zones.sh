#!/usr/bin/env bash
# PreToolUse on Edit|Write|MultiEdit|NotebookEdit: a subagent writes only
# inside its role's zone, as .claude/zones declares it.
set -u
in=$(cat)
role=$(jq -r '.agent_type // empty' <<<"$in")
[ -n "$role" ] || exit 0
file=$(jq -r '.tool_input.file_path // .tool_input.notebook_path // empty' <<<"$in")
[ -n "$file" ] || exit 0
cwd=$(jq -r '.cwd' <<<"$in")
root=$(git -C "$cwd" rev-parse --show-toplevel 2>/dev/null) || exit 0
root=$(realpath -m -- "$root")
case $file in /*) ;; *) file=$cwd/$file ;; esac
file=$(realpath -m -- "$file")
case $file in
  "$root"/*) rel=${file#"$root"/} ;;
  *) exit 0 ;;
esac
while read -r owner pattern _; do
  [ "$owner" = "$role" ] || continue
  re=${pattern//./\\.}; re=${re//\*\*/@}; re=${re//\*/[^/]*}; re=${re//@/.*}
  [[ $rel =~ ^${re}$ ]] && exit 0
done < <(grep -Ev '^[[:space:]]*(#|$)' "$root/.claude/zones")
jq -n --arg r "$rel is outside the $role zone (.claude/zones): hand the change to the role that owns it" \
  '{hookSpecificOutput: {hookEventName: "PreToolUse", permissionDecision: "deny", permissionDecisionReason: $r}}'
