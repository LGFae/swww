#!/bin/sh

# This script generates man pages in a `doc/generated` directory. In order to
# install these, you need to move the man pages to the appropriate location in
# your system. You should be able to figure out the correct directories by
# running `manpath`.
#
# Package Maintainers: please consult your distribution's specific documentation
# to adapt to whatever idiosyncrasies it may have.

set -e

DIR=$(dirname "$0")
GEN_DIR="$DIR"/generated

if [ ! -d "$GEN_DIR" ]; then
	mkdir -v "$GEN_DIR"
fi

for FILE in "$DIR"/*scd; do
	GEN="$GEN_DIR"/"$(basename --suffix .scd "$FILE")"
	printf "generating %s..." "$GEN"
	scdoc < "$FILE" > "$GEN"
	printf " ...done!\n"
done
