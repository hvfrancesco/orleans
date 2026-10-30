# orleans

[![CI](https://github.com/hvfrancesco/orleans/actions/workflows/ci.yml/badge.svg)](https://github.com/hvfrancesco/orleans/actions/workflows/ci.yml)

An independent, clean-room CAM stack for laser cutters, written in Rust. It reads
vector designs and raster images, plans toolpaths and streams G-code to
GRBL-compatible and Marlin laser controllers over USB or WiFi. Everything is
implemented from public specifications: SVG, PDF, DXF, G-code, GRBL and Marlin.

> **Status:** `1.2.0`. The editor and GRBL workflow are hardware-validated.
> Marlin laser support is covered by simulator and TCP integration tests but
> awaits validation on a physical Marlin machine. See [CHANGELOG.md](CHANGELOG.md)
> and [docs/roadmap.md](docs/roadmap.md).

## Highlights

| Area | What it does |
|---|---|
| Input | SVG, PDF and PDF-compatible AI, DXF, PNG/JPEG/BMP |
| Design | Layers, fills, kerf, offsets, booleans, tabs, groups, arrays, node editing |
| Machine | GRBL, GrblHAL, FluidNC and Marlin lasers over USB serial or WiFi TCP/Telnet |
| Control | Status, homing, unlock, jog, work zero, framing and stop; pause/resume on GRBL |
| Output | G-code export, live estimates, persistent job history |
| Interface | Dark/light themes and six languages: English, Italian, French, Spanish, German, Chinese |
| Projects | Native `.orleans` snapshots with full geometry and raster data |

The codebase is split into small crates with a strict dependency direction, and
the full pipeline — SVG → toolpath → G-code → TCP controller — is covered by
integration tests against local GRBL and Marlin servers.

## Quick start

Requires a recent stable Rust toolchain and a C++ compiler (Clipper2 is built
from source).

```bash
cargo build --release

# Open the visual editor; the input is optional
./target/release/orleans gui examples/coaster.svg

# Inspect a design: size, layers, cut length and time estimate
./target/release/orleans info examples/coaster.svg

# Dry-run the whole pipeline against the built-in simulator
./target/release/orleans run examples/coaster.svg --mock

# Dry-run the Marlin dialect against the Marlin simulator
./target/release/orleans run examples/coaster.svg --mock --driver marlin

# Cut over WiFi (Telnet on port 23 is the default)
./target/release/orleans run examples/coaster.svg \
    --url tcp://192.168.1.50 --speed 1200 --power 35
```

See [docs/cli.md](docs/cli.md) for all commands, address formats and examples.
Read [docs/gui.md](docs/gui.md) for the editor workflow, and
[docs/troubleshooting.md](docs/troubleshooting.md) before connecting to a
machine for the first time.

## Documentation

| Document | Contents |
|---|---|
| [Release notes](CHANGELOG.md) | What changed in each release |
| [GUI guide](docs/gui.md) | Editor, layers, transforms, fills, raster import, machine panel |
| [CLI reference](docs/cli.md) | Commands, options and examples |
| [Project format](docs/project-format.md) | Native `.orleans` JSON structure |
| [Troubleshooting](docs/troubleshooting.md) | WiFi, protocol detection and recovery |
| [Development](docs/development.md) | Architecture, design decisions and known limitations |
| [Releasing](docs/releasing.md) | Versioning, packaging, checksums and tagging |
| [Roadmap](docs/roadmap.md) | Release checklist and planned milestones |

## Architecture

```
crates/
  orleans-core/      document model: Document → Layer → Shape, CutParams, Job
  orleans-geom/      flattening, area, containment, Clipper2 offsets/booleans
  orleans-import/    SVG/PDF/DXF → Document, system-font text, raster images
  orleans-toolpath/  Document → Job: nesting, ordering, passes, time estimate
  orleans-driver/    Job → G-code, Transport trait, serial/TCP, GRBL and Marlin clients, simulators
  orleans-cli/       the `orleans` binary
```

The crates depend only downwards. The machine-specific code lives in
`orleans-driver`; adding a controller does not change the document model or the
editor. See [docs/development.md](docs/development.md) for the full architecture
and design notes.

## Project files

`.orleans` is a self-contained snapshot of the editor document: page size, job
settings, layers, paths, groups, fill/tab settings and compressed raster images.
It is the native project format and does not depend on re-importing an SVG or
DXF. The current format is **2**; the layout is documented in
[docs/project-format.md](docs/project-format.md).

## Building and testing

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --release
```

Release packaging is reproducible:

```bash
./scripts/package-release.sh   # binary archive + SHA-256
./scripts/package-appimage.sh  # Linux AppImage + SHA-256
```

The suite currently contains **190 tests**, including the full SVG → toolpath →
G-code → TCP controller integration run and a Marlin TCP worker test.

The GUI is behind the default `gui` feature. Build with `--no-default-features`
for a headless binary without the windowing stack.

## License

Released under the MIT or Apache-2.0 license. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).

The interface uses [Phosphor Icons](https://phosphoricons.com/), available under
the MIT or Apache-2.0 license.
