# Releasing

## Version and notes

1. Set the workspace version in `Cargo.toml`.
2. Update `CHANGELOG.md` with a dated release entry.
3. Update the status and test count in `README.md` if needed.
4. Update [roadmap.md](roadmap.md) checkboxes.
5. Run the checks:

   ```bash
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   cargo fmt --all -- --check
   cargo build --release
   ```

## Build the package

```bash
python3 scripts/package-portable.py
```

The script reads the workspace version and host target, builds with
`--release --locked`, and writes a host-native archive:

```text
dist/orleans-<version>-<target>.tar.gz       # Linux, macOS
dist/orleans-<version>-<target>.zip          # Windows
dist/orleans-<version>-<target>.*.sha256     # matching checksum
```

The archive contains the binary, README, changelog, licenses, documentation and
the sample coaster design. Verify a Unix archive with:

```bash
cd dist
sha256sum -c orleans-<version>-<target>.tar.gz.sha256
./orleans-<version>-<target>/orleans --version
```

On Windows, verify the `.zip.sha256` file with `certutil -hashfile` or
PowerShell `Get-FileHash`, then extract the zip.

The older `scripts/package-release.sh` remains available for Unix-only builds.

## Build the AppImage

```bash
./scripts/package-appimage.sh
```

This builds an AppDir with `linuxdeploy` and writes:

```text
dist/orleans-<version>-<target>.AppImage
dist/orleans-<version>-<target>.AppImage.sha256
```

Launching the AppImage without arguments opens the editor. Explicit CLI commands
and `--help`/`--version` are passed through unchanged.

The AppImage uses the host X11/Wayland stack, OpenGL drivers, system fonts and
`libudev` for USB serial access. It is not sandboxed. USB serial access therefore depends only on the host's
kernel driver and device permissions, exactly like the plain binary. See
[troubleshooting.md](troubleshooting.md) for serial permissions.

Run the smoke test without mounting FUSE:

```bash
APPIMAGE_EXTRACT_AND_RUN=1 \
  ./dist/orleans-<version>-<target>.AppImage run examples/coaster.svg --mock
```

## GitHub Actions

- `.github/workflows/ci.yml` runs formatting, Clippy and tests on Ubuntu, and
  native builds on Windows, macOS Intel and macOS Apple Silicon for pushes to
  `main` and pull requests.
- `.github/workflows/release.yml` runs on `v*` tags. It runs the checks on
  Ubuntu, then builds and smoke-tests native Linux, Windows and macOS packages.
  The Linux job also builds the AppImage. All archives and checksums are
  uploaded and published together.

The release workflow uses the repository's `GITHUB_TOKEN`, so the workflow
permissions must allow `contents: write`.

## Tag and publish

After the package and documentation are final:

```bash
git add .
git commit -m "Release orleans <version>"
git tag -a v<version> -m "orleans <version>"
```

Push `main` and the tag:

```bash
git push origin main
git push origin v<version>
```

The release workflow builds from the tagged commit and publishes the Linux
archive, AppImage, Windows zip and both macOS archives with their checksums.
Do not upload an archive rebuilt after tagging.

Windows and macOS binaries are unsigned. macOS may require removing the
quarantine attribute, and Windows may show a SmartScreen warning on first
launch.
