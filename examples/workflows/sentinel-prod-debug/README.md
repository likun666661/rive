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
