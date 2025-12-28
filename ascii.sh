#!/usr/bin/env bash

# --- Configuration ---

# The ratio between the width and height of the font used for rendering.
FONT_RATIO="0.44"

# Any pixel with a brightness below this value (0-255) will be treated as transparent.
# Increase this to remove more of the dark areas from the image.
LUMINANCE_THRESHOLD=30

# A string of characters to represent pixels, from darkest to lightest.
# The backtick is escaped with a backslash (\`) to prevent a syntax error.
ASCII_CHARS=" .'\`^,:;Il!i><~+_-?][}{1)(|/tfjrxnuvczXYUJCLQ0OZmwqpdbkhao*#MW&8%B@$"

# Video processing settings
VIDEO_FORMATS=("mp4" "mkv" "mov" "avi")
OUTPUT_FPS=30
OUTPUT_COLUMNS=80

# --- Functions ---

#
# Outputs the appropriate ASCII character for a given RGB color based on its luminance.
#
# @param $1: The r,g,b Pixel (e.g., "255,215,0")
#
pixel_for() {
    local r g b
    IFS=',' read -r r g b <<< "$1"

    # Calculate relative luminance (a measure of brightness) from 0-255.
    local luminance
    luminance=$(awk -v r="$r" -v g="$g" -v b="$b" 'BEGIN{print int(0.2126*r + 0.7152*g + 0.0722*b)}')

    # If the pixel's brightness is below our threshold, render it as a blank space.
    if (( luminance < LUMINANCE_THRESHOLD )); then
        echo -n " "
        return
    fi

    # Map the remaining luminance range (THRESHOLD to 255) to our character set
    # to maintain full contrast on the parts of the image we want to see.
    local num_chars=${#ASCII_CHARS}
    local effective_luminance=$((luminance - LUMINANCE_THRESHOLD))
    local luminance_range=$((255 - LUMINANCE_THRESHOLD))

    # Prevent division by zero if threshold is set to 255
    if (( luminance_range <= 0 )); then luminance_range=1; fi

    local char_index=$(( (effective_luminance * (num_chars - 1)) / luminance_range ))

    # Print the character at the calculated index.
    echo -n "${ASCII_CHARS:$char_index:1}"
}

#
# Extracts frames from a video, converts them to text, and processes them into ASCII art.
#
# @param $1: The video file
# @param $2: The directory to place the output files
#
generate_frame_images() {
    local video_file="$1"
    local working_dir="$2"
    local frame_images_dir="$working_dir/frame_images"
    mkdir -p "$frame_images_dir"

    echo "Extracting frames from '$video_file'..."
    ffmpeg \
        -loglevel error \
        -i "$video_file" \
        -vf "scale=$OUTPUT_COLUMNS:-2,fps=$OUTPUT_FPS" \
        "$frame_images_dir/frame_%04d.png"

    echo "Processing frames into ASCII..."
    for f in $(find "$frame_images_dir" -name '*.png' | sort); do
        local squished_image_file="${f%.png}_squished.png"
        local image_height
        image_height=$(magick identify -ping -format '%h' "$f")
        local new_height
        new_height=$(awk -v ratio="$FONT_RATIO" -v height="$image_height" 'BEGIN{print int(ratio * height + 0.5)}')

        magick "$f" -resize "x$new_height"'!' "$squished_image_file"

        local imagemagick_text_file="${f%.png}_im.txt"
        local output_text_file="${f%.png}.txt"

        magick "$squished_image_file" "$imagemagick_text_file"

        local last_row=-1
        tail -n +2 "$imagemagick_text_file" | while read -r line; do
            local xy_part="${line%% *}"
            local rgb_part="${line#*srgb(}"
            local rgb="${rgb_part%')'}"
            local row="${xy_part#*,}"
            row="${row%:}"

            if [[ "$row" != "$last_row" ]]; then
                if (( last_row != -1 )); then
                    echo "" >> "$output_text_file"
                fi
                last_row=$row
            fi

            pixel_for "$rgb" >> "$output_text_file"
        done
        echo "" >> "$output_text_file"

        rm "$f" "$squished_image_file" "$imagemagick_text_file"
        echo "Processed ${f##*/}"
    done
    echo "ASCII generation complete."
}

#
# Main function to orchestrate the video-to-ASCII conversion.
#
# @param $1: The path to the video file
#
video_to_terminal() {
    local video_file="$1"
    if [[ -z "$video_file" ]]; then
        >&2 echo "Error: No input file specified."
        >&2 echo "Usage: $0 <path_to_video_file>"
        return 1
    fi

    if [[ ! -f "$video_file" ]]; then
        >&2 echo "Error: Input file '$1' does not exist."
        return 1
    fi

    local file_extension
    file_extension="$(echo "${video_file##*.}" | awk '{print tolower($0)}')"
    if [[ ! " ${VIDEO_FORMATS[*]} " =~ " ${file_extension} " ]]; then
        >&2 echo "Error: Unsupported file format '$file_extension'."
        >&2 echo "Supported formats: ${VIDEO_FORMATS[*]}"
        return 1
    fi

    local working_dir="./ascii_frames_$(date +%s)"
    mkdir "$working_dir"
    echo "Created working directory: $working_dir"

    generate_frame_images "$video_file" "$working_dir"

    echo "All frames processed. You can find the output .txt files in '$working_dir/frame_images/'"
    echo "To play the animation, you can use a command like:"
    echo "watch -n $OUTPUT_FPS --no-title 'cat $working_dir/frame_images/\`ls $working_dir/frame_images/ | sort | head -n \$(( (\$SECONDS % \$(ls $working_dir/frame_images/ | wc -l)) + 1 )) | tail -n 1\`'"
}

# --- Execution ---
video_to_terminal "$1"
