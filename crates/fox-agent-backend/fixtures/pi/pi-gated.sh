#!/bin/sh
# pi command for pi-acp (set PI_ACP_PI_COMMAND to this file). Runs the pi
# binary Fox pins (FOX_PI_BIN, e.g. services/agent-runtime/node_modules/.bin/pi,
# @earendil-works/pi-coding-agent 0.84.2) with extension discovery off and only
# the Fox approval gate loaded. pi-acp appends `--mode rpc --no-themes
# [--session <file>]`; extra flags for tests come from FOX_PI_EXTRA_ARGS
# (word-split, test use only).
# Unix only. Windows: pi-gated.cmd next to this file (pi-acp spawns *.cmd
# through cross-spawn).
DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
# shellcheck disable=SC2086
exec "${FOX_PI_BIN:-pi}" --no-extensions -e "$DIR/fox-approval-gate.ts" ${FOX_PI_EXTRA_ARGS:-} "$@"
