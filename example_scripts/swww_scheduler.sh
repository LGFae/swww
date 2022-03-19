#!/bin/sh

# This is a script to help you schedule image switching at different times of
# the day. You may use it as-is or as inspiration for something else

if [ $# -lt 2 ]; then
	echo "Usage:
	$0 <path/to/img [optional arguments to pass to swww]> <time in HH:MM format>

This will use the 'at' command to schedule the image switch.
You can control the transition fps or step by passing the respective options:

	$0 'path/to/img --transition-fps 60 --transition-step 5' '18:00'
"
	exit 1
fi

if ! type "at" > /dev/null 2>&1; then
	echo "ERROR: 'at' command doesn't exist!"
	exit 1
fi

echo "swww img $1" | at "$2"
