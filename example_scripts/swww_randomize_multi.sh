#!/bin/bash

# This script will randomly go through the files of a directory,
# setting a different random wallpaper for each display
# at regular intervals
#
# NOTE: this script is in bash (not posix shell), because the RANDOM variable
# we use is not defined in posix

if [[ $# -lt 1 ]] || [[ ! -d $1 ]]; then
    echo "Usage:
    $0 <dir containing images>"
    exit 1
fi

# Make sure only 1 instance of swww_randomize
PIDFILE=~/.local/state/swww-randomize-pidfile.txt
if [ -e "${PIDFILE}" ]; then
    OLD_PID="$(<${PIDFILE})"
    if [ "X" != "X${OLD_PID}" -a -e "/proc/${OLD_PID}" ]; then
        OLD_NAME="$(</proc/${OLD_PID}/comm)"
        THIS_NAME="$(</proc/${BASHPID}/comm)"
        if [ "${OLD_NAME}" = "${THIS_NAME}" ]; then
            echo "old randomize process ${OLD_PID} is still running"
            exit 1
        else
            echo "process with same ID as old randomize is running: \"${OLD_NAME}\"@${OLD_PID}"
            echo "Replacing old process ID"
        fi
    fi
fi
echo "${BASHPID}" > ${PIDFILE}

# Edit below to control the images transition
export SWWW_TRANSITION_FPS=60
export SWWW_TRANSITION_STEP=2

# This controls (in seconds) when to switch to the next image
INTERVAL=300

# Possible values:
#    -   no:   Do not resize the image
#    -   crop: Resize the image to fill the whole screen, cropping out parts that don't fit
#    -   fit:  Resize the image to fit inside the screen, preserving the original aspect ratio
RESIZE_TYPE="fit"

DISPLAY_LIST=$(swww query | grep -Po "^[^:]+")

while true; do
    find "$1" -type f \
        | while read -r img; do
            echo "$RANDOM:$img"
        done \
        | sort -n | cut -d':' -f2- \
        | tee ~/.local/state/swww-randomize-list.txt \
        | while read -r img; do
            # Set a different image for each display
            for disp in $DISPLAY_LIST; do
                # if there is no image try to get one
                if [ "X" = "X${img}" ]; then
                    if read -r img; then
                        true
                    else # if there are no more images, refresh the list
                        break 2
                    fi
                fi
                swww img --resize=$RESIZE_TYPE --outputs $disp $img
                # make sure each image is only used once
                img=""
            done
            sleep $INTERVAL
        done
done
