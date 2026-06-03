# Sentinel Production Debug Workflow

This is a reusable Rive workflow package. It contains both the workflow DAG and the prompt templates needed to instantiate that DAG.

Import:

```sh
rive workflow import examples/workflows/sentinel-prod-debug --command-id import-sentinel-v1
```

Instantiate without starting the scheduler:

```sh
rive workflow run sentinel.prod-debug \
  --param slack_channel=#incidents \
  --param env=prd \
  --param since=1h \
  --command-id run-sentinel-v1 \
  --no-scheduler
```

Shape:

```text
root
  -> global-signal-scan
  -> investigate-alva-backend
  -> investigate-alfs
  -> investigate-jagent
  -> investigate-alva-gateway
  -> final-judge-and-slack

final-judge-and-slack depends on all investigation nodes.
```

GitHub issue creation and Slack posting are gated by boolean parameters and default to dry-run behavior.

## VM Cron Example

`run-sentinel-cron.sh` is an example deployment wrapper for a VM that should run this workflow on a timer.

It keeps two semantics separate:

- A normal timer tick starts a fresh `rive workflow run sentinel.prod-debug`.
- If the latest workflow run is still incomplete, the script resumes its scheduler instead of starting another overlapping investigation.

The script is intentionally outside Rive core. It is a practical operator wrapper around the existing CLI:

```sh
RIVE_BIN=/opt/rive/bin/rive \
CODEX_BIN=/usr/local/bin/codex \
RIVE_SENTINEL_WORKSPACE=/srv/mono-meta \
RIVE_SENTINEL_WORKFLOW_PACKAGE=/opt/rive/examples/workflows/sentinel-prod-debug \
RIVE_SENTINEL_WORKER=sentinel-codex-worker \
RIVE_SENTINEL_ENV=prd \
RIVE_SENTINEL_SINCE=30m \
RIVE_SENTINEL_SLACK_CHANNEL='#alerts' \
examples/workflows/sentinel-prod-debug/run-sentinel-cron.sh
```

The wrapper:

- initializes the Rive workspace if needed;
- imports this workflow package if missing;
- creates the configured worker agent if missing;
- uses a lock directory under `.rive/run/` so cron cannot overlap runs;
- starts or resumes a Codex-backed scheduler run;
- waits for root Work DAG projection to finish;
- locates the `final-judge-and-slack` artifact from the Rive ledger.

Slack notification is deterministic and disabled by default. The workflow itself still runs with `allow_slack_post=false`, so agents only produce a Slack draft. To send a real Slack message after Rive confirms root `done`, enable the outer notifier and provide a URL base for the final markdown artifact:

```sh
RIVE_SENTINEL_NOTIFY_SLACK=1 \
SENTINEL_REPORT_BASE_URL=https://internal.example.com/mono-meta \
SENTINEL_SLACK_SCRIPT=/srv/mono-meta/users/kunli/sentinel/slack-notify.sh \
examples/workflows/sentinel-prod-debug/run-sentinel-cron.sh
```

The existing Sentinel Slack script accepts only `--title` and `--url`, so the VM must publish `reports/sentinel/...` somewhere Slack can open. If no URL base is configured, the wrapper leaves the final markdown on disk and prints its path.

For cron, copy and edit `crontab.example`.
