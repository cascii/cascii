#!/bin/bash
set -e

# App specific directory in Application Support
APP_SUPPORT_DIR="$HOME/Library/Application Support/cascii"
REPO_DIR="$APP_SUPPORT_DIR/repo"

echo "Setting up application directory at $APP_SUPPORT_DIR..."
mkdir -p "$REPO_DIR"

echo "Copying repository to $REPO_DIR..."
rsync -a --delete --exclude='.git' --exclude='target' ./ "$REPO_DIR/"

echo "Building release binary in $REPO_DIR..."
(cd "$REPO_DIR" && cargo build --release)

INSTALL_DIR="/usr/local/bin"

# Install cascii binary
BINARY_NAME="cascii"
SOURCE_PATH="$REPO_DIR/target/release/$BINARY_NAME"
echo "Installing $BINARY_NAME to $INSTALL_DIR..."
sudo cp "$SOURCE_PATH" "$INSTALL_DIR/$BINARY_NAME"
echo "Creating backward-compatible 'casci' symlink..."
sudo ln -sf "$INSTALL_DIR/$BINARY_NAME" "$INSTALL_DIR/casci"

# Install default config if not present
CONFIG_DIR="$APP_SUPPORT_DIR"
CONFIG_PATH="$CONFIG_DIR/cascii.json"
if [ ! -f "$CONFIG_PATH" ]; then
    echo "Installing default config to $CONFIG_PATH..."
    mkdir -p "$CONFIG_DIR"
    cp "$REPO_DIR/resources/cascii.json" "$CONFIG_PATH"
fi

# Determine shell configuration file
SHELL_CONFIG=""
if [[ "$SHELL" == */zsh ]]; then
    SHELL_CONFIG="$HOME/.zshrc"
elif [[ "$SHELL" == */bash ]]; then
    SHELL_CONFIG="$HOME/.bash_profile"
else
    echo "Unsupported shell: $SHELL."
    exit 1
fi

# Ensure the shell configuration file exists
touch "$SHELL_CONFIG"

echo "Installation complete."
echo "You can now use 'cascii'."
