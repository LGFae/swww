#!/bin/bash

WALLPAPER_DIR="$HOME/.config/hypr/wallpapers/"

INTERVAL=300

ALLOWED_EXTENSIONS=("jpg" "jpeg" "png")

while true; do
    # Find images
    images=()
    for ext in "${ALLOWED_EXTENSIONS[@]}"; do
        images+=($(find $WALLPAPER_DIR -type f -iname "*.$ext"))
    done

    if [ ${#images[@]} -eq 0 ]; then
        echo "No images were found in this folder $WALLPAPER_DIR with allowed extensions"
        exit 1
    fi

    # Choose random image as wallpaper
    random_image="${images[RANDOM % ${#images[@]}]}"

    # Set background image
    swww img "$random_image" 
    # Time
    sleep $INTERVAL
done

