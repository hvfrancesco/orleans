# Troubleshooting

## Before the first connection

1. Confirm the laser is off or the controller is in a safe state.
2. Set the machine work origin before sending a job.
3. Run a framing pass or a laser-off dry run before the first cut.
4. Keep physical access to the machine's emergency stop.

## WiFi and Telnet checks

```bash
orleans status --url tcp://IP
```

A healthy controller prints a status line such as:

```text
<Idle|MPos:0.000,0.000,0.000|FS:0,0>
```

If the TCP connection opens but the controller stays silent, orleans retries the
request, also sends a newline-terminated query (some bridges are line-buffered),
then probes `$I` and `M115` to identify the firmware and prints everything it
received.

Common causes:

- **The laser controller is off or asleep** while its WiFi bridge is still
  powered. The bridge accepts TCP; the GRBL board behind it is not listening.
- **Something else is using port 23.** Check `nc -vz <ip> 23` and the router's
  client list.
- **The firmware is not GRBL.** The probe reports the firmware family; orleans
  currently streams GRBL only.
- **The bridge is WebSocket-only on port 81.** This transport is not implemented
  yet.

## A job stops unexpectedly

`orleans run` refuses to stream when no status comes back; `--force` overrides
that check.

If a job aborts while the link is still alive, `orleans` sends feed hold followed
by a soft reset. This stops the planner and switches the laser off. If the link
itself died, stop the machine at the panel.

After any reset:

1. Check the machine and workpiece.
2. Re-home or re-zero as appropriate; a soft reset returns GRBL to its startup
   state.
3. Run a framing pass before retrying the job.

## Timeouts

`--timeout` (default 30 s) is a **no-response** timeout. The job is aborted only
if no `ok`, status or other line arrives for that long. It is not a limit on the
total job duration: a long cut with steady acknowledgements is fine.

Raise the timeout for a controller that can stay silent for longer while
executing one very slow move.

## G-code looks wrong

- Confirm the design units and page size in **info**.
- Kerf is applied to the toolpath, not displayed as an offset on the canvas.
- Curves are flattened at import/plan time, so detailed designs produce more
  G-code.
- If holes and outer contours are on different layers, check the layer cut order.
- Text imported from SVG or DXF is outlined with the host's system fonts; missing
  or SHX fonts fall back to sans-serif.

## Interface language or fonts

The language preference is stored in:

```text
$XDG_CONFIG_HOME/orleans/ui.json
```

or `~/.config/orleans/ui.json`. Chinese text needs a system CJK font such as Noto
Sans CJK or Source Han Sans.
