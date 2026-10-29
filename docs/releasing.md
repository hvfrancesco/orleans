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

## Tag and publish

After the package and documentation are final:

```bash
git add .
git commit -m "Release orleans <version>"
git tag -a v<version> -m "orleans <version>"
```

Publish the archive, its checksum and the release notes together. Do not publish
an archive that has been rebuilt after tagging; rebuild it from the tagged commit
instead.
