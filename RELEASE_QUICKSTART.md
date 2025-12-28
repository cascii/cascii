# Quick Start: Publishing cascii

## ✅ You're Ready!

All the automation is now set up. Here's what you need to do:

## 1. Initial Setup (One-time)

### Get your crates.io token:
1. Create account at https://crates.io
2. Get API token from https://crates.io/me

### Add GitHub Secret:
1. Go to your GitHub repo → Settings → Secrets and variables → Actions
2. Add new secret: `CARGO_REGISTRY_TOKEN` = your crates.io token

### Update repository URL in Cargo.toml:
Replace `https://github.com/cascii/cascii` with your actual repo URL.

## 2. To Publish a New Version

### Option A: Auto-Release via Commit Message (⚡ Fastest)

Simply commit to master with a special commit message format:

```bash
# Format: release(*): <description>
# Examples:
git commit -m "release(0.2.1): Fixed ASCII character encoding bug"
git commit -m "release: Added video conversion feature"
git commit -m "release(v0.3.0): Major refactor - library support"
git push
```

**That's it!** When you push to master with a commit starting with `release(*):`, the workflow will automatically:
- ✅ Extract version from Cargo.toml
- ✅ Create a git tag (e.g., v0.2.1)
- ✅ Create a GitHub Release with your commit message as release notes

**Note:** Multi-line commit messages work too! Everything after the first line becomes the release notes:
```bash
git commit -m "release(0.2.1): Major updates" -m "
- Fixed encoding bug
- Added new features
- Improved performance
"
git push
```

### Option B: Manual Tag-Based Release

```bash
# 1. Update version in Cargo.toml (currently at 0.2.0)
# 2. Commit everything
git add .
git commit -m "Release v0.2.0"
git push

# 3. Create and push tag
git tag v0.2.0
git push origin v0.2.0
```

**This triggers the full release workflow:**
**This triggers the full release workflow:**
- ✅ Build binaries for macOS and Linux
- ✅ Create a GitHub Release
- ✅ Publish to crates.io

## 3. Others Can Use It

Once published, anyone can add to their `Cargo.toml`:

```toml
[dependencies]
cascii = "0.3"
```

## What Got Set Up

### ✅ GitHub Workflows Created:

**`.github/workflows/auto-release.yml`** - Auto-release on special commits:
- Triggers on commits to master with message like `release(*): ...`
- Automatically creates git tag from Cargo.toml version
- Creates GitHub Release with commit message as notes
- ⚡ Fastest way to release!

**`.github/workflows/ci.yml`** - Runs on every push/PR:
- Tests on multiple platforms
- Checks code formatting
- Runs clippy linter
- Ensures version is bumped in PRs

**`.github/workflows/release.yml`** - Runs when you push a tag:
- Builds for macOS (ARM64 & x64) and Linux (x64 & ARM64)
- Creates GitHub Release with binaries
- Publishes to crates.io

### ✅ Files Added:

- `LICENSE-MIT` & `LICENSE-APACHE` - Dual license
- `PUBLISHING.md` - Detailed publishing guide
- `LIBRARY.md` - How to use as a library
- Updated `Cargo.toml` with all metadata
- `examples/` - Usage examples

### ✅ Library Features:

Your package now includes:
- Public API (`src/lib.rs`)
- Documentation with examples
- Both CLI and library functionality

## Testing Before Release (Optional)

```bash
# Test package creation (won't actually publish)
cargo package --allow-dirty

# See what files will be included
cargo package --list --allow-dirty
```

## After First Publish

Users can install the CLI:
```bash
cargo install cascii
```

Or use as a library:
```toml
[dependencies]
cascii = "0.3"
```

## Need Help?

See `PUBLISHING.md` for detailed documentation including:
- Troubleshooting
- Pre-release versions
- Version management
- Best practices

---

**You're all set! Just commit, tag, and push to release! 🚀**

