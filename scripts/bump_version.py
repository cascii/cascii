#!/usr/bin/env python3
"""
Automatic version bumping script for cascii.
Bumps version in Cargo.toml based on commit message prefix.

Usage:
    python3 scripts/bump_version.py <commit_message> [base_version]

Arguments:
    commit_message  - The commit message to determine bump type
    base_version    - Optional: The version to bump from (for upgrade scenarios)

Commit message prefixes:
    release(...): ...   -> Bumps major version (X.0.0)
    feature(...): ...   -> Bumps minor version (0.X.0)
    fix(...): ...       -> Bumps patch version (0.0.X)
    anything else       -> No version bump
"""

import sys
import re
from pathlib import Path

def parse_version(version_str):
    """Parse version string into (major, minor, patch) tuple."""
    match = re.match(r'(\d+)\.(\d+)\.(\d+)', version_str)
    if match:
        return tuple(map(int, match.groups()))
    raise ValueError(f"Invalid version format: {version_str}")

def bump_version(version_str, bump_type):
    """Bump version based on type: 'major', 'minor', or 'patch'."""
    major, minor, patch = parse_version(version_str)

    if bump_type == 'major':
        return f"{major + 1}.0.0"
    elif bump_type == 'minor':
        return f"{major}.{minor + 1}.0"
    elif bump_type == 'patch':
        return f"{major}.{minor}.{patch + 1}"
    else:
        return version_str

def get_bump_type_from_commit(commit_msg):
    """Determine bump type from commit message."""
    commit_msg = commit_msg.strip().lower()

    if commit_msg.startswith('release('):
        return 'major'
    elif commit_msg.startswith('feature('):
        return 'minor'
    elif commit_msg.startswith('fix('):
        return 'patch'
    else:
        return None

def get_current_version(cargo_toml_path):
    """Extract current version from Cargo.toml."""
    content = cargo_toml_path.read_text()
    match = re.search(r'^version\s*=\s*"([^"]+)"', content, re.MULTILINE)
    if match:
        return match.group(1)
    raise ValueError("Could not find version in Cargo.toml")

def update_cargo_toml(file_path, new_version):
    """Update version in Cargo.toml - only the [package] section version."""
    content = file_path.read_text()
    lines = content.split('\n')
    updated_lines = []
    in_package_section = False
    version_updated = False

    for line in lines:
        if line.strip() == '[package]':
            in_package_section = True
            updated_lines.append(line)
            continue

        if in_package_section and line.strip().startswith('[') and line.strip() != '[package]':
            in_package_section = False

        if in_package_section and re.match(r'^\s*version\s*=\s*"[^"]+"', line):
            updated_lines.append(re.sub(r'(version\s*=\s*")[^"]+(")', rf'\g<1>{new_version}\g<2>', line))
            version_updated = True
        else:
            updated_lines.append(line)

    if version_updated:
        file_path.write_text('\n'.join(updated_lines))
        return True
    return False

def main():
    if len(sys.argv) < 2:
        print("Usage: bump_version.py <commit_message> [base_version]", file=sys.stderr)
        sys.exit(1)

    commit_msg = sys.argv[1]
    base_version = sys.argv[2] if len(sys.argv) > 2 else None

    bump_type = get_bump_type_from_commit(commit_msg)

    if not bump_type:
        sys.exit(0)

    # Paths
    root_dir = Path(__file__).parent.parent
    cargo_toml = root_dir / 'Cargo.toml'

    # Get current version
    current_version = get_current_version(cargo_toml)

    # Use base_version if provided (bump from original version), otherwise use current
    version_to_bump = base_version if base_version else current_version

    # Bump version
    new_version = bump_version(version_to_bump, bump_type)

    # Skip if the new version equals current (already at this version)
    if new_version == current_version:
        print(f"No version change: {current_version}")
        sys.exit(0)

    # Update Cargo.toml
    if update_cargo_toml(cargo_toml, new_version):
        print(f"Version bumped: {version_to_bump} -> {new_version} ({bump_type})")
        print(f"Updated: Cargo.toml")
        sys.exit(0)
    else:
        print(f"No changes made for version: {current_version}")
        sys.exit(0)

if __name__ == '__main__':
    main()
