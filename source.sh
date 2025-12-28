#!/usr/bin/env bash

# Parameters
IMAGE="hanyu.png"
OUTPUT="hanyu.txt"
COLUMNS=400
FONT_RATIO=0.7
LUMINANCE_THRESHOLD=20
ASCII_CHARS=" .'\`^,:;Il!i><~+_-?][}{1)(|/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$"

if [ ! -f "$IMAGE" ]; then
    echo "Error: $IMAGE not found"
    exit 1
fi

# Get dimensions
WIDTH=$(magick identify -format "%w" "$IMAGE")
HEIGHT=$(magick identify -format "%h" "$IMAGE")

# Calculate new height to maintain aspect ratio with font correction
# NewHeight = (Height / Width) * Columns * FontRatio
NEW_HEIGHT=$(awk -v h="$HEIGHT" -v w="$WIDTH" -v c="$COLUMNS" -v r="$FONT_RATIO" 'BEGIN { printf("%.0f", (h/w) * c * r) }')

echo "Converting $IMAGE ($WIDTH x $HEIGHT) -> ASCII ($COLUMNS x $NEW_HEIGHT)"

# Resize and convert to ASCII
# We use awk for faster processing than shell loops
magick "$IMAGE" \
    -resize "${COLUMNS}x${NEW_HEIGHT}!" \
    txt:- | \
    awk -v thresh="$LUMINANCE_THRESHOLD" \
        -v chars="$ASCII_CHARS" \
        '
    BEGIN {
        # Split char string into array (1-based index in awk)
        n = split(chars, c, "")
        range = 255 - thresh
        if (range <= 0) range = 1
        last_y = 0
    }
    
    # Process lines that contain pixel data (skip header/comments)
    # Format usually: x,y: (r,g,b) ...
    # We match lines starting with a number
    /^[0-9]/ {
        # Parse x,y from $1 (e.g., "0,0:")
        split($1, coords, ",")
        x = coords[1]
        y = substr(coords[2], 1, length(coords[2])-1) # remove trailing colon
        
        # Handle newlines
        if (y != last_y) {
            printf "\n"
            last_y = y
        }
        
        # Parse RGB from $2 (e.g., "(28,28,28)")
        # Remove parens
        rgb_str = substr($2, 2, length($2)-2)
        split(rgb_str, rgb, ",")
        r = rgb[1]
        g = rgb[2]
        b = rgb[3]
        
        # Calculate luminance
        lum = int(0.2126*r + 0.7152*g + 0.0722*b)
        
        if (lum < thresh) {
            printf " "
        } else {
            effective_lum = lum - thresh
            # Map to char index (0 to n-1)
            # Formula from bash script: ((effective * (num - 1)) / range)
            idx = int((effective_lum * (n - 1)) / range) + 1
            printf "%s", c[idx]
        }
    }
    END {
        printf "\n"
    }
    ' > "$OUTPUT"

echo "Done. Output saved to $OUTPUT"

