#!/bin/bash
# Report source files exceeding a line threshold.
# Informational — does not fail the build. Use to track growth.
#
# Usage: scripts/check_file_size.sh [max_lines]

MAX_LINES="${1:-800}"
count=0

while IFS= read -r file; do
    lines=$(wc -l < "$file")
    if [ "$lines" -gt "$MAX_LINES" ]; then
        printf "  %5d  %s\n" "$lines" "$file"
        count=$((count + 1))
    fi
done < <(find src -name '*.rs' -not -path '*/tests/*' | sort)

if [ "$count" -gt 0 ]; then
    echo "  ─────"
    echo "  $count files over ${MAX_LINES} lines"
else
    echo "  all files under ${MAX_LINES} lines"
fi
