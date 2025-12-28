# Publishing cascii to crates.io

This guide explains how to publish `cascii` to crates.io and set up automated releases.

## Prerequisites

1. **crates.io account**: Create an account at https://crates.io
2. **API Token**: Get your token from https://crates.io/me
3. **GitHub repository**: Push your code to GitHub

## Initial Setup

### 1. Update Cargo.toml metadata

Ensure these fields are filled in `Cargo.toml`:

```toml
[package]
name = "cascii"
version = "0.3.0"
edition = "2021"
description = "High-performance ASCII art generator for images and videos"
license = "MIT OR Apache-2.0"
repository = "https://github.com/YOUR_USERNAME/cascii"
keywords = ["ascii", "art", "video", "image", "converter"]
categories = ["multimedia::video", "multimedia::images", "command-line-utilities"]
readme = "README.md"
homepage = "https://github.com/YOUR_USERNAME/cascii"
```

### 2. Add a LICENSE file

Choose a license and add LICENSE-MIT and/or LICENSE-APACHE files to your repo.

### 3. Set up GitHub Secrets

Add these secrets to your GitHub repository (Settings → Secrets and variables → Actions):

- `CARGO_REGISTRY_TOKEN`: Your crates.io API token

## Publishing Workflow

### Option 1: Automated (Recommended)

The GitHub Actions workflow will automatically publish to crates.io when you create a release tag:

```bash
# 1. Update version in Cargo.toml
# 2. Commit and push
git add Cargo.toml
git commit -m "Bump version to 0.3.0"
git push

# 3. Create and push a tag
git tag v0.3.0
git push origin v0.3.0
```

This will:
- Build binaries for multiple platforms
- Create a GitHub Release
- Publish to crates.io automatically

### Option 2: Manual

Publish manually from your local machine:

```bash
# Login to crates.io (first time only)
cargo login YOUR_TOKEN

# Publish
cargo publish
```

## Using cascii as a Dependency

Once published, others can use cascii by adding to their `Cargo.toml`:

```toml
[dependencies]
cascii = "0.3"
```

Or for the latest version:

```toml
[dependencies]
cascii = "0.3.0"
```

## Version Management

### Semantic Versioning

Follow semver (https://semver.org/):
- **MAJOR** (1.0.0): Breaking changes
- **MINOR** (0.3.0): New features, backward compatible
- **PATCH** (0.3.1): Bug fixes

### Version Bump Workflow

1. **Update Cargo.toml version**
2. **Update CHANGELOG** (optional but recommended)
3. **Commit**: `git commit -m "Bump version to X.Y.Z"`
4. **Tag**: `git tag vX.Y.Z`
5. **Push**: `git push && git push --tags`

The CI workflow will check that the version is bumped in PRs.

## Continuous Integration

The repository includes two workflows:

### CI Workflow (`.github/workflows/ci.yml`)

Runs on every push and PR:
- Runs tests
- Checks formatting
- Runs clippy (linter)
- Builds examples
- Verifies version bump in PRs

### Release Workflow (`.github/workflows/release.yml`)

Runs when you push a tag:
- Builds for multiple platforms
- Creates GitHub Release with binaries
- Publishes to crates.io

## Troubleshooting

### "crate name is already taken"

If the name is taken, update the `name` field in `Cargo.toml`.

### "missing required field"

Ensure all required fields in `Cargo.toml` are filled:
- name, version, edition
- license or license-file
- description

### "failed to publish"

Check:
- Your token is valid
- The version doesn't already exist on crates.io
- All dependencies are published to crates.io (no path dependencies in published version)

## Best Practices

1. **Always bump version** before releasing
2. **Test locally** before publishing: `cargo publish --dry-run`
3. **Keep README updated** with installation instructions
4. **Document breaking changes** in release notes
5. **Use pre-release versions** for testing: `0.3.0-beta.1`

## Pre-release Testing

Test a pre-release version:

```bash
# Set pre-release version
# Cargo.toml: version = "0.3.0-beta.1"

# Publish
cargo publish

# Users can test with:
# [dependencies]
# cascii = "0.3.0-beta.1"
```

## After Publishing

1. **Verify on crates.io**: Check https://crates.io/crates/cascii
2. **Update documentation**: Ensure docs.rs builds correctly
3. **Announce**: Share on social media, Reddit r/rust, etc.
4. **Monitor issues**: Respond to user feedback

## Documentation

docs.rs will automatically build and host documentation from your published crate.

View at: https://docs.rs/cascii

To test documentation locally:

```bash
cargo doc --open
```

## Resources

- [Cargo Publishing Guide](https://doc.rust-lang.org/cargo/reference/publishing.html)
- [crates.io Policies](https://crates.io/policies)
- [Semantic Versioning](https://semver.org/)
- [GitHub Actions Documentation](https://docs.github.com/en/actions)

