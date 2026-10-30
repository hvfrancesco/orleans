# Troubleshooting

## Before the first connection

1. Confirm the laser is off or the controller is in a safe state.
2. Set the machine work origin before sending a job.
3. Run a framing pass or a laser-off dry run before the first cut.
4. Keep physical access to the machine's emergency stop.

## AppImage and USB serial

The AppImage is not sandboxed, so WiFi and USB access work like the plain
binary. USB serial still requires:

- the host kernel driver (`ch341`, `ftdi_sio`, `cdc_acm`, etc.) for the board;
- read/write permission on `/dev/ttyUSB*` or `/dev/ttyACM*`;
- membership in `dialout` on Debian/Ubuntu, or `uucp` on Arch and similar
  distributions, followed by a new login session.

Check the device and permissions with:

```bash
ls -l /dev/ttyUSB0
id
```

The AppImage does not bundle GPU drivers or system CJK fonts. It uses the host
X11/Wayland stack, OpenGL drivers and installed fonts.

## WiFi and Telnet checks

```bash
orleans status --url tcp://IP
```

A healthy GRBL controller prints a status line such as:

```text
<Idle|MPos:0.000,0.000,0.000|FS:0,0>
```

`orleans status` and the GUI auto-detect the firmware: first they request GRBL
status with `?`, then they send Marlin's `M115` identity query. When a protocol
is recognised, orleans prints `protocol: GRBL` or `protocol: Marlin`; Marlin
also prints the `M114` position and `M105` temperature replies.

If auto-detection fails but the controller is known to be GRBL or Marlin, force
it with the CLI's `--driver grbl|marlin`. In the GUI, disconnect and choose the
same protocol in the **Driver** selector below **Endpoint**, then connect again.
Forced mode is only a protocol override: normal status, jogging and streaming
still require the controller to answer its usual status commands.

Common causes:

- **The laser controller is off or asleep** while its WiFi bridge is still
  powered. The bridge accepts TCP; the GRBL board behind it is not listening.
- **Something else is using port 23.** Check `nc -vz <ip> 23` and the router's
  client list.
- **The firmware is not GRBL or Marlin.** Auto-detection accepts GRBL status
  reports and Marlin `M115` identities; Ruida, TopWisdom and galvo controllers
  are not implemented yet.
- **The bridge is WebSocket-only on port 81.** This transport is not implemented
  yet.

## A job stops unexpectedly

`orleans run` refuses to stream when no status comes back; `--force` overrides
that check.

If a GRBL job aborts while the link is still alive, `orleans` sends feed hold
followed by a soft reset. This stops the planner and switches the laser off. A
Marlin abort sends `M5` and then `M112` (emergency stop). If the link itself
died, stop the machine at the panel.

After a GRBL reset, or after a Marlin stop:

1. Check the machine and workpiece.
2. On Marlin, press **Unlock (M999)** or run `orleans unlock` to restart the
   halted controller.
3. Re-home or re-zero as appropriate; a reset returns GRBL to its startup state
   and clears Marlin's temporary `G92` origin.
4. Run a framing pass before retrying the job.

## Timeouts

`--timeout` (default 30 s) is a **no-response** timeout. The job is aborted only
if no `ok`, status or other line arrives for that long. It is not a limit on the
total job duration: a long cut with steady acknowledgements is fine.

Raise the timeout for a controller that can stay silent for longer while
executing one very slow move.

## Marlin-specific checks

- Marlin has no GRBL `$` settings. The GUI assumes a `0..255` PWM range; set
  **Max S** to match the firmware if it uses a different range.
- Pause is not available on Marlin. Use Stop (`M5` + `M112`) and then Unlock
  (`M999`).
- **Save zero (G54)** is disabled because Marlin only supports the temporary
  `G92` work origin.
- For the fastest emergency stop, enable Marlin's `EMERGENCY_PARSER` so `M112`
  is handled without waiting for queued moves. Even without it, orleans sends
  `M5` before `M112`.
- Marlin `ok` acknowledges a command entering the planner. Orleans sends
  `M400` after a job to wait for the planner to finish before reporting it done.

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
