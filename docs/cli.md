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
| `unlock` | Clear a GRBL alarm with `$X` |

## Addresses

Machine commands accept USB serial or TCP connections.

```text
--port /dev/ttyUSB0        USB serial, GRBL defaults to 115200 baud
--url tcp://host[:port]    WiFi TCP/Telnet, port 23 by default
--url telnet://host[:port]
--url host[:port]
```

Examples:

```bash
# Status over USB or WiFi
orleans status --port /dev/ttyUSB0
orleans status --url tcp://192.168.1.50

# Emergency stop: feed hold + soft reset, laser off
orleans stop --url tcp://192.168.1.50

# Clear a GRBL alarm after a failed job
orleans unlock --url tcp://192.168.1.50
```

## Examples

```bash
# Open the visual editor (the input is optional)
orleans gui examples/coaster.svg --url tcp://192.168.1.10

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

# Dry-run the whole pipeline against the built-in simulator
orleans run examples/coaster.svg --mock

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

If a job aborts while the link is still alive, `orleans` sends feed hold followed
by a soft reset, which stops the planner and switches the laser off. If the link
itself died, stop the machine at the panel. Check the machine and re-home or
re-zero before retrying; a soft reset returns GRBL to its startup state.

See [troubleshooting.md](troubleshooting.md) for WiFi diagnostics.
