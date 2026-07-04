---
name: macos-sandbox-timed-app-run
description: "Run an app for N seconds in Claude Code on macOS: no timeout binary, foreground sleep blocked, mkdir scratchpad first"
user-invocable: false
origin: auto-extracted
---

# Timed app runs on macOS in the Claude Code sandbox

**Extracted:** 2026-07-04
**Context:** Verifying a change by running a windowed or long-lived app for a
fixed duration (e.g. "run the game for 25 s and check the log for panics") on macOS.

## Problem

Three environment quirks break the obvious approaches, some silently:

1. `timeout 25 cargo run` fails — macOS ships no coreutils `timeout` (exit 127;
   with output redirected the failure just looks like an empty log).
2. Foreground `sleep` (including chained short sleeps) is blocked by the harness.
3. The advertised session scratchpad directory may not exist yet — a
   `> $SCRATCHPAD/run.log` redirect then fails before the command runs, so the
   app never starts at all.

## Solution

Put everything inside ONE background job (`run_in_background: true`), where
`sleep` is allowed, and create the log directory first:

```bash
LOG=<scratchpad>/run.log
mkdir -p "$(dirname "$LOG")"
APP_ENV=1 cargo run > "$LOG" 2>&1 & pid=$!
sleep 25; kill $pid 2>/dev/null; sleep 2
echo "panics: $(grep -c panicked "$LOG")"; tail -8 "$LOG"
```

Wait for the task-completion notification, or poll with an until-loop:

```bash
until [ -s <task-output-file> ]; do sleep 3; done; cat <task-output-file>
```

## When to Use

- Any "run the app for N seconds and inspect its output" verification on macOS
- Anywhere you would reach for `timeout`/`gtimeout` on Linux
- Before redirecting output into the session scratchpad for the first time
