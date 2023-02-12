#!/bin/sh

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
