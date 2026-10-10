#!/usr/bin/env bash
# Extract the smaps classes printed by --measure (OPENPDF_MEASURE_RSS_BREAKDOWN=1) as
# rss_heap_kb/rss_tiles_kb/rss_pdfium_kb/rss_other_kb, plus rss_mesa_kb summed from the
# top-15 rss_map_* lines whose path matches mesa/gallium/llvm/lavapipe. Prints nothing if absent.
log="$1"
[ -f "$log" ] || exit 0
u=$(grep -oE 'ui_first_tile_ms *= *[0-9.]+' "$log" | tail -1 | tr -d ' ' | cut -d= -f2); [ -n "$u" ] && echo "ui_first_tile_ms=$u"
grep -q '^rss_breakdown=1' "$log" || exit 0
v() { grep -oE "^rss_class_$1_kb=[0-9]+" "$log" | tail -1 | cut -d= -f2; }
for k in heap tiles textures; do x=$(v $k); [ -n "$x" ] && echo "rss_${k}_kb=$x"; done
x=$(v libpdfium); [ -n "$x" ] && echo "rss_pdfium_kb=$x"
x=$(v other); [ -n "$x" ] && echo "rss_other_kb=$x"
m=$(grep -E '^rss_map_[0-9]+=' "$log" | grep -iE 'mesa|gallium|llvm|lvp|vulkan_lvp|dri' | awk '{s+=$2} END {print s+0}')
echo "rss_mesa_kb=$m"
