#!/usr/bin/env bash
# Run or resume the reusable Sentinel production-debug workflow.
#
# Intended deployment:
#   - Put this script on a VM that has Rive, Codex, sentinel-cli, jq, and sqlite3.
#   - Trigger it from cron or systemd timer every 30 minutes.
#   - Keep Slack notification deterministic: Rive produces a final artifact, then
#     this script calls Sentinel's Slack delivery script with a title + URL.
#
# Safe defaults:
#   - GitHub writes and Slack posts are disabled inside the agent workflow.
#   - The outer Slack notifier is disabled unless RIVE_SENTINEL_NOTIFY_SLACK=1.

set -euo pipefail

log() {
  printf '[sentinel-rive-cron] %s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

sql_quote() {
  printf "%s" "$1" | sed "s/'/''/g"
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

workspace="${RIVE_SENTINEL_WORKSPACE:-$(pwd)}"
workflow_package="${RIVE_SENTINEL_WORKFLOW_PACKAGE:-$script_dir}"
template_id="${RIVE_SENTINEL_TEMPLATE_ID:-sentinel.prod-debug}"
worker="${RIVE_SENTINEL_WORKER:-sentinel-codex-worker}"
env_name="${RIVE_SENTINEL_ENV:-prd}"
since="${RIVE_SENTINEL_SINCE:-30m}"
slack_channel="${RIVE_SENTINEL_SLACK_CHANNEL:-#alerts}"
max_parallel="${RIVE_SENTINEL_MAX_PARALLEL:-3}"
timeout_seconds="${RIVE_SENTINEL_TIMEOUT_SECONDS:-1800}"
codex_bin="${CODEX_BIN:-/usr/local/bin/codex}"
rive_bin="${RIVE_BIN:-rive}"
sentinel_bin_dir="${SENTINEL_BIN_DIR:-$workspace/users/kunli/sentinel/cli/bin}"
sentinel_skill_dir="${SENTINEL_SKILL_DIR:-$workspace/users/kunli/sentinel}"
notify_slack="${RIVE_SENTINEL_NOTIFY_SLACK:-0}"
slack_script="${SENTINEL_SLACK_SCRIPT:-$workspace/users/kunli/sentinel/slack-notify.sh}"
report_base_url="${SENTINEL_REPORT_BASE_URL:-}"
allow_github_write="${RIVE_SENTINEL_ALLOW_GITHUB_WRITE:-false}"
allow_slack_post="${RIVE_SENTINEL_ALLOW_SLACK_POST:-false}"
resume_stale="${RIVE_SENTINEL_RESUME_STALE:-1}"

export SENTINEL_SKILL_DIR="$sentinel_skill_dir"
export PATH="$sentinel_bin_dir:$PATH"
if [[ "$rive_bin" == */* ]]; then
  rive_bin_dir="$(dirname "$rive_bin")"
  export PATH="$rive_bin_dir:$PATH"
fi

need_cmd "$rive_bin"
need_cmd sentinel
need_cmd jq
need_cmd sqlite3
[[ -x "$codex_bin" ]] || die "codex binary not executable: $codex_bin"
[[ -d "$workspace" ]] || die "workspace does not exist: $workspace"
[[ -f "$sentinel_skill_dir/SKILL.md" ]] || die "Sentinel skill not found: $sentinel_skill_dir/SKILL.md"

cd "$workspace"

if [[ ! -d .rive ]]; then
  log "initializing Rive workspace at $workspace"
  "$rive_bin" init "$workspace" >/dev/null
fi

db_path="$workspace/.rive/rive.db"
lock_dir="$workspace/.rive/run/sentinel-prod-debug.lock"
if ! mkdir "$lock_dir" 2>/dev/null; then
  log "another Sentinel workflow runner appears active: $lock_dir"
  exit 0
fi
trap 'rmdir "$lock_dir" 2>/dev/null || true' EXIT

if ! "$rive_bin" workflow show "$template_id" >/dev/null 2>&1; then
  import_id="sentinel-prod-debug-import-$(date -u +%Y%m%d%H%M%S)"
  log "importing workflow package $workflow_package"
  "$rive_bin" workflow validate "$workflow_package" >/dev/null
  "$rive_bin" workflow import "$workflow_package" --command-id "$import_id" >/dev/null
fi

if ! "$rive_bin" agent list | jq -e --arg worker "$worker" \
  '.protocol.agents[]? | select(.name == $worker and .role == "worker")' >/dev/null; then
  log "creating worker agent $worker"
  "$rive_bin" agent add "$worker" --role worker >/dev/null
fi

timestamp="$(date -u +%Y%m%d%H%M%S)"
run_json=""
workflow_run_id=""
scheduler_run_id=""

if [[ "$resume_stale" == "1" ]]; then
  stale_row="$(sqlite3 "$db_path" "
select workflow_run_id || '|' || coalesce(scheduler_run_id, '__none__') || '|' || root_work_node_id
from workflow_runs
where template_id = '$(sql_quote "$template_id")'
  and state not in ('completed', 'failed')
order by created_at desc
limit 1;
")"
  IFS='|' read -r stale_workflow stale_scheduler stale_root <<<"$stale_row"
  if [[ "${stale_scheduler:-}" == "__none__" ]]; then
    stale_scheduler=""
  fi
else
  stale_workflow=""
  stale_scheduler=""
  stale_root=""
fi

if [[ -n "${stale_workflow:-}" ]]; then
  log "resuming stale workflow_run=$stale_workflow scheduler=${stale_scheduler:-<none>} root=$stale_root"
  if [[ -n "${stale_scheduler:-}" ]]; then
    run_json="$("$rive_bin" scheduler resume \
      --run "$stale_scheduler" \
      --worker "$worker" \
      --command-id "sentinel-resume-$timestamp" \
      --max-parallel "$max_parallel" \
      --acceptance-mode auto-reported \
      --workspace-mode shared \
      --codex-bin "$codex_bin" \
      --trust-project \
      --timeout-seconds "$timeout_seconds")"
  else
    run_json="$("$rive_bin" scheduler resume \
      --root "$stale_root" \
      --worker "$worker" \
      --command-id "sentinel-resume-$timestamp" \
      --max-parallel "$max_parallel" \
      --acceptance-mode auto-reported \
      --workspace-mode shared \
      --codex-bin "$codex_bin" \
      --trust-project \
      --timeout-seconds "$timeout_seconds")"
  fi
  workflow_run_id="$stale_workflow"
  scheduler_run_id="$(printf '%s' "$run_json" | jq -r '.protocol.scheduler.scheduler_run_id // .protocol.scheduler_run_id // empty')"
else
  command_id="sentinel-run-$timestamp"
  log "starting new workflow run command_id=$command_id"
  run_json="$("$rive_bin" workflow run "$template_id" \
    --param "env=$env_name" \
    --param "since=$since" \
    --param "slack_channel=$slack_channel" \
    --param "allow_github_write=$allow_github_write" \
    --param "allow_slack_post=$allow_slack_post" \
    --command-id "$command_id" \
    --runner codex \
    --worker "$worker" \
    --max-parallel "$max_parallel" \
    --acceptance-mode auto-reported \
    --workspace-mode shared \
    --codex-bin "$codex_bin" \
    --trust-project \
    --timeout-seconds "$timeout_seconds")"
  workflow_run_id="$(printf '%s' "$run_json" | jq -r '.protocol.workflow_run_id')"
  scheduler_run_id="$(printf '%s' "$run_json" | jq -r '.protocol.scheduler.scheduler_run_id // empty')"
fi

printf '%s\n' "$run_json"

if [[ -z "$workflow_run_id" || "$workflow_run_id" == "null" ]]; then
  die "could not determine workflow_run_id"
fi

if [[ -z "${scheduler_run_id:-}" || "$scheduler_run_id" == "null" ]]; then
  scheduler_run_id="$(sqlite3 "$db_path" "
select coalesce(scheduler_run_id, '')
from workflow_runs
where workflow_run_id = '$(sql_quote "$workflow_run_id")';
")"
fi

workflow_state="$(sqlite3 "$db_path" "select state from workflow_runs where workflow_run_id = '$(sql_quote "$workflow_run_id")';")"
scheduler_state=""
root_state=""
if [[ -n "${scheduler_run_id:-}" && "$scheduler_run_id" != "null" ]]; then
  status_json="$("$rive_bin" scheduler status --run "$scheduler_run_id" || true)"
  scheduler_state="$(printf '%s' "$status_json" | jq -r '.protocol.scheduler.state // empty')"
  root_state="$(printf '%s' "$status_json" | jq -r '.protocol.root_work.state // empty')"
fi

if [[ "$workflow_state" != "completed" && ! ( "$scheduler_state" == "completed" && "$root_state" == "done" ) ]]; then
  log "workflow_run=$workflow_run_id ended with state=$workflow_state scheduler_state=${scheduler_state:-unknown} root_state=${root_state:-unknown}; skipping Slack notification"
  if [[ -n "$scheduler_run_id" && "$scheduler_run_id" != "null" ]]; then
    "$rive_bin" scheduler status --run "$scheduler_run_id" || true
  fi
  exit 1
fi

final_artifact="$(sqlite3 "$db_path" "
select wrb.artifact_ref
from workflow_run_nodes wrn
join work_ref_bindings wrb on wrb.work_node_id = wrn.work_node_id
where wrn.workflow_run_id = '$(sql_quote "$workflow_run_id")'
  and wrn.node_template_id = 'final-judge-and-slack'
  and wrb.artifact_ref is not null
order by wrb.id desc
limit 1;
")"

[[ -n "$final_artifact" ]] || die "workflow completed but final artifact was not found"
[[ -f "$final_artifact" ]] || die "final artifact path not found: $final_artifact"

title="$(awk '/^# / { sub(/^# /, ""); print; exit }' "$final_artifact")"
title="${title:-Sentinel $env_name workflow result since $since}"

log "completed workflow_run=$workflow_run_id scheduler_run=${scheduler_run_id:-unknown}"
log "final artifact: $final_artifact"

if [[ "$notify_slack" == "1" ]]; then
  [[ -x "$slack_script" ]] || die "Slack script not executable: $slack_script"
  [[ -n "$report_base_url" ]] || die "SENTINEL_REPORT_BASE_URL is required when RIVE_SENTINEL_NOTIFY_SLACK=1"
  report_url="${report_base_url%/}/${final_artifact#./}"
  log "posting Slack notification title='$title' url=$report_url"
  "$slack_script" --title "$title" --url "$report_url"
else
  log "Slack notification disabled; set RIVE_SENTINEL_NOTIFY_SLACK=1 and SENTINEL_REPORT_BASE_URL to enable"
fi
