# Auto-Release Workflow Guide

This workflow automatically creates a GitHub release when you commit to master with a specific commit message format.

## How It Works

When you push a commit to `master` with a message starting with `release(*):`, the workflow:

1. ✅ Detects the special commit message
2. ✅ Extracts the version from `Cargo.toml`
3. ✅ Creates a git tag (e.g., `v0.2.0`)
4. ✅ Creates a GitHub Release with your commit message as release notes

## Commit Message Format

The pattern is: `release(*): <your message>`

Where `*` can be anything (or nothing).

### Valid Examples:

```bash
# Simple
git commit -m "release: Fixed bug in ASCII conversion"

# With version in parens
git commit -m "release(0.2.1): Fixed bug in ASCII conversion"

# With scope
git commit -m "release(lib): Added library support"

# With v prefix
git commit -m "release(v0.2.1): Fixed bug in ASCII conversion"
```

### Multi-line Commit Messages

Everything after the first line becomes the release notes:

```bash
git commit -m "release(0.2.1): Major updates" -m "
## What's New

- Fixed ASCII character encoding bug
- Added new video conversion features  
- Improved performance by 50%

## Breaking Changes

- Changed API signature for convert_image()
"
```

## Complete Example

```bash
# 1. Make your changes
git add .

# 2. Bump version in Cargo.toml (if needed)
# Change: version = "0.2.0" to version = "0.2.1"

# 3. Commit with release message
git commit -m "release(0.2.1): Fixed critical encoding bug"

# 4. Push to master
git push origin master
```

**Done!** Check the Actions tab to see the release being created.

## What Gets Created

After the workflow runs, you'll have:

- **Git Tag**: `v0.2.1` (matches Cargo.toml version)
- **GitHub Release**: With your commit message as release notes
- **Assets**: The release is created (binaries come from the separate release workflow if triggered by tag)

## Viewing the Release

Go to: `https://github.com/YOUR_USERNAME/cascii/releases`

## Tips

### ✅ DO:
- Update `Cargo.toml` version before committing
- Use descriptive release messages
- Include bullet points for multiple changes
- Test locally before pushing

### ❌ DON'T:
- Forget to bump the version in Cargo.toml
- Use the pattern for non-release commits
- Create conflicting tags manually

## Checking if It Will Trigger

Test the pattern locally:

```bash
# Check your commit message
COMMIT_MSG="release(0.2.1): Test"
echo "$COMMIT_MSG" | grep -qE "^release(\(.*\))?:\s+.+" && echo "✅ Will trigger!" || echo "❌ Won't trigger"
```

## What If It Doesn't Work?

1. **Check the pattern**: Must start with `release` (case-sensitive)
2. **Check the branch**: Must be pushed to `master`
3. **Check Actions tab**: See if workflow ran and any errors
4. **Version conflict**: If tag already exists, workflow will replace it

## Disabling This Workflow

If you want to disable this temporary workflow:

```bash
# Rename or delete the file
git mv .github/workflows/auto-release.yml .github/workflows/auto-release.yml.disabled
git commit -m "Disable auto-release workflow"
git push
```

Or delete it entirely:

```bash
git rm .github/workflows/auto-release.yml
git commit -m "Remove auto-release workflow"
git push
```

## Advanced: Custom Release Notes Template

You can customize the release notes by editing the workflow file (`.github/workflows/auto-release.yml`).

For example, to add a header:

```yaml
- name: Extract release notes from commit
  run: |
    COMMIT_MSG="${{ github.event.head_commit.message }}"
    BODY=$(git log -1 --pretty=%B | tail -n +2)
    
    # Add custom header
    echo "## Release v${{ steps.version.outputs.version }}" > release_notes.txt
    echo "" >> release_notes.txt
    echo "$BODY" >> release_notes.txt
```

## FAQ

**Q: Can I use this for pre-releases?**
A: Yes! Just update the workflow to set `prerelease: true` in the release step.

**Q: What if I make a mistake?**
A: You can delete the release and tag from GitHub, then push again.

**Q: Can I trigger the full build workflow too?**
A: Yes! The auto-release creates a tag, which will also trigger the main `release.yml` workflow for building binaries.

**Q: Does this publish to crates.io?**
A: No, this only creates a GitHub release. For crates.io publishing, the tag will trigger the main release workflow.

---

**This is a TEMPORARY convenience workflow.** Once you're comfortable with the process, you may want to use the standard tag-based release workflow instead.

