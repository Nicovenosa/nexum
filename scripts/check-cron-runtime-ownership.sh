#!/usr/bin/env bash
set -euo pipefail

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(git -C "$script_dir" rev-parse --show-toplevel 2>/dev/null || dirname "$script_dir")
unexpected=0

while IFS= read -r match; do
    case "$match" in
        # The standalone host is the sole production owner of CronRuntime.
        "$root"/nexum-acp-host/src/host.rs:*) ;;
        # Runtime and legacy scheduler tests may construct isolated fixtures.
        "$root"/nexum-acp/src/cron/mod_test.rs:*) ;;
        "$root"/nexum-middlewares/src/cron/mod_test.rs:*) ;;
        "$root"/nexum-middlewares/src/cron/tools_test.rs:*) ;;
        *)
            printf 'Cron runtime ownership violation: %s\n' "$match" >&2
            unexpected=1
            ;;
    esac
done < <(
    rg -n --glob '*.rs' \
        'CronScheduler::new|spawn_tick_task|poll_cron_triggers|CronRuntime::new' \
        "$root" \
        --glob '!target/**' || true
)

if [ "$unexpected" -ne 0 ]; then
    exit 1
fi
