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
./scripts/package-release.sh
```

The script reads the workspace version and host target, builds with
`--release --locked`, and writes:

```text
dist/orleans-<version>-<target>.tar.gz
dist/orleans-<version>-<target>.tar.gz.sha256
```

The archive contains the binary, README, changelog, licenses, documentation and
the sample coaster design. Verify it with:

```bash
cd dist
sha256sum -c orleans-<version>-<target>.tar.gz.sha256
./orleans-<version>-<target>/orleans --version
```

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

The AppImage uses the host X11/Wayland stack, OpenGL drivers and system fonts.
It is not sandboxed. USB serial access therefore depends only on the host's
kernel driver and device permissions, exactly like the plain binary. See
[troubleshooting.md](troubleshooting.md) for serial permissions.

Run the smoke test without mounting FUSE:

```bash
APPIMAGE_EXTRACT_AND_RUN=1 \
  ./dist/orleans-<version>-<target>.AppImage run examples/coaster.svg --mock
```

## GitHub Actions

- `.github/workflows/ci.yml` runs formatting, Clippy and tests on pushes to
  `main` and on pull requests.
- `.github/workflows/release.yml` runs on `v*` tags, rebuilds the archive and
  AppImage, smoke-tests them, uploads workflow artifacts and publishes the
  GitHub release with both checksums.

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

The release workflow builds from the tagged commit and publishes the archive,
AppImage and both checksums together. Do not upload an archive rebuilt after
tagging.
