#!/bin/sh

# This is a helper script to use just before releasing a new version
# (it helps us not forget anything, as has happenned before)

if [ $# -lt 1 ]; then
	echo "Usage: $0 <new version name>"
	exit 1
fi

pkill swww-daemon

set -e

# don't forget testing everything
cargo build
cargo test -- --include-ignored
./doc/gen.sh # make sure the docs "compile"

# Cargo.toml:
sed "s/^version = .*/version = \"$1\"/" Cargo.toml > TMP \
	&& mv TMP Cargo.toml

# CHANGELOG:
sed -e "s/^### Unreleased/### $1/" \
	-e '1s/^/### Unreleased\n\n\n/' CHANGELOG.md > TMP \
	&& mv TMP CHANGELOG.md

# Make sure it still builds (just to be 100% safe), and to update Cargo.lock
cargo build
