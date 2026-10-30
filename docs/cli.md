# CLI reference

The `orleans` binary has commands for inspecting designs, generating G-code,
streaming jobs and controlling a machine. Run `orleans <command> --help` for the
full option list.

## Commands

| Command | Purpose |
|---|---|
| `gui` | Open the visual editor |
| `info` | Inspect a design and the job it would produce |
| `gcode` | Convert a design to G-code |
| `run` | Stream a design to a controller |
| `status` | Ask a connected controller for its status |
| `stop` | Stop the machine now and turn the laser off |
| `unlock` | Clear a GRBL alarm (`$X`) or restart halted Marlin (`M999`) |

## Addresses

Machine commands accept USB serial or TCP connections.

```text
--port /dev/ttyUSB0        USB serial; controllers default to 115200 baud
--url tcp://host[:port]    WiFi TCP/Telnet, port 23 by default
--url telnet://host[:port]
--url host[:port]
--url ws://host:port/ws    ESP3D / WebSocket terminal bridge
```

## Controller selection

`run`, `stop`, `unlock` and `status` accept `--driver`. `gcode` also accepts it
for an offline export; `auto` means GRBL there because there is no controller to
probe:

```text
--driver auto      probe for GRBL status, then Marlin M115 identity (default)
--driver grbl      force the GRBL / GrblHAL / FluidNC driver
--driver marlin    force the Marlin laser driver
```

Auto-detection uses GRBL's realtime `?` status first and Marlin's `M115`
firmware identity second. Marlin status is built from `M114` position and
`M105` temperature replies.

For Marlin, generated G-code uses the portable Marlin preamble (no `G17` or
`G54`) and defaults **Max S** to `255` instead of GRBL's `1000`. Override it with
`--max-s` if the firmware uses another PWM range.

Examples:

```bash
# Status over USB or WiFi; the protocol is detected automatically
orleans status --port /dev/ttyUSB0
orleans status --url tcp://192.168.1.50

# ESP3D terminal WebSocket
orleans status --url ws://192.168.1.50:81/ws

# Emergency stop: GRBL feed hold + soft reset, laser off
orleans stop --url tcp://192.168.1.50

# Clear a GRBL alarm after a failed job
orleans unlock --url tcp://192.168.1.50

# Marlin laser over USB, with the default 0..255 PWM range
orleans run design.svg --port /dev/ttyUSB0 --driver marlin

# Emergency stop on Marlin (M112), then restart it
orleans stop --port /dev/ttyUSB0 --driver marlin
orleans unlock --port /dev/ttyUSB0 --driver marlin
```

## Examples

```bash
# Open the visual editor (the input is optional)
orleans gui examples/coaster.svg --url tcp://192.168.1.10

# Prefill a USB Marlin connection
orleans gui examples/coaster.svg --port /dev/ttyUSB0 --baud 115200 --driver marlin

# Open a DXF drawing
orleans gui drawing.dxf

# Open a PDF or a PDF-compatible Illustrator file
orleans gui drawing.pdf

# Import a specific PDF page (1-based)
orleans info drawing.pdf --page 2

# Open a PNG/JPEG/BMP as a raster-engraving layer
orleans gui photo.png

# Inspect a design: size, layers, cut length and time estimate
orleans info examples/coaster.svg

# Generate G-code
orleans gcode examples/coaster.svg -o coaster.gcode \
    --speed 1200 --power 35 --kerf 0.15 --passes 2

# Generate G-code for a Marlin laser (portable preamble, max S 255)
orleans gcode examples/coaster.svg -o coaster-marlin.gcode \
    --driver marlin --speed 1200 --power 35

# Dry-run the whole pipeline against the built-in GRBL simulator
orleans run examples/coaster.svg --mock

# Dry-run the Marlin dialect against the Marlin simulator
orleans run examples/coaster.svg --mock --driver marlin

# Cut for real over USB
orleans run examples/coaster.svg --port /dev/ttyUSB0 \
    --speed 1200 --power 35

# Cut over WiFi
orleans run examples/coaster.svg --url tcp://192.168.1.50 \
    --speed 1200 --power 35
```

## Run behaviour

`orleans run` refuses to stream when no status comes back; `--force` overrides
that check.

`--timeout` (default 30 s) is a **no-response** timeout: the job is aborted only
if no `ok`, status or other line arrives for that long. It is not a limit on the
total job duration, so a 20-minute cut with steady acknowledgements is fine.
Raise it for a controller that can stay silent for longer while executing one
very slow move.

If a GRBL job aborts while the link is still alive, `orleans` sends feed hold
followed by a soft reset, which stops the planner and switches the laser off. A
Marlin abort sends `M5` and then `M112` (emergency stop); run `orleans unlock`
(`M999`) before continuing. If the link itself died, stop the machine at the
panel. Check the machine and re-home or re-zero before retrying.

Marlin has no safe equivalent of GRBL feed hold, so `pause` is not supported by
the GUI for Marlin jobs; use Stop instead.

See [troubleshooting.md](troubleshooting.md) for WiFi diagnostics.
