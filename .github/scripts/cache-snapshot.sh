#!/usr/bin/env bash
# List ~/.cache (and system fontconfig cache) recursively: "<bytes>\t<path>" per entry.
set -u
out="$1"
{
  for d in "$HOME/.cache" /var/cache/fontconfig; do
    if [ -d "$d" ]; then
      find "$d" -printf '%s\t%y\t%p\n' 2>/dev/null
    else
      printf '0\tmissing\t%s\n' "$d"
    fi
  done
} | LC_ALL=C sort -k3 > "$out"
echo "snapshot $out: $(wc -l < "$out") entries"
