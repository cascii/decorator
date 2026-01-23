#!/bin/bash

# Prevent recursive execution
if [ "$DECORATOR_AUTO_VERSIONING" == "1" ]; then
    exit 0
fi

# Get last commit message
MSG=$(git log -1 --pretty=%B)

# Check for keywords
BUMP=""
# Check for "feature" (case insensitive)
if echo "$MSG" | grep -iq "feature"; then
    BUMP="minor"
# Check for "fix" (case insensitive)
elif echo "$MSG" | grep -iq "fix"; then
    BUMP="patch"
fi

if [[ -n "$BUMP" ]]; then
    # Get current version from src-tauri/Cargo.toml
    CURRENT_VERSION=$(grep "^version =" src-tauri/Cargo.toml | head -n 1 | cut -d '"' -f 2)

    if [[ -z "$CURRENT_VERSION" ]]; then
        echo "Could not find version in src-tauri/Cargo.toml"
        exit 0
    fi

    IFS='.' read -r major minor patch <<< "$CURRENT_VERSION"

    if [[ "$BUMP" == "minor" ]]; then
        minor=$((minor + 1))
        patch=0
    elif [[ "$BUMP" == "patch" ]]; then
        patch=$((patch + 1))
    fi

    NEW_VERSION="$major.$minor.$patch"
    echo "Auto-bumping version ($BUMP) from $CURRENT_VERSION to $NEW_VERSION"

    # Update src-tauri/Cargo.toml
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" src-tauri/Cargo.toml
        # Update root Cargo.toml (frontend)
        sed -i '' "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml
        # Update tauri.conf.json
        sed -i '' "s/\"version\": \"$CURRENT_VERSION\"/\"version\": \"$NEW_VERSION\"/" src-tauri/tauri.conf.json
    else
        sed -i "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" src-tauri/Cargo.toml
        sed -i "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" Cargo.toml
        sed -i "s/\"version\": \"$CURRENT_VERSION\"/\"version\": \"$NEW_VERSION\"/" src-tauri/tauri.conf.json
    fi

    # Update Cargo.lock by running a quick check
    cargo check --manifest-path src-tauri/Cargo.toml > /dev/null 2>&1

    # Stage the files and amend the commit
    git add src-tauri/Cargo.toml Cargo.toml src-tauri/tauri.conf.json Cargo.lock

    export DECORATOR_AUTO_VERSIONING=1
    git commit --amend --no-edit --allow-empty
fi
