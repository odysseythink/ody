#!/usr/bin/env bash
# Final mechanism demo: emulate the ConPTY frame tail byte-for-byte.
# Phase A: text batch ending on the bottom row, then \e[?25h + \e[25l (ConPTY's
#          broken hide) and HOLD — this is the state WT is in whenever the
#          app's re-park CUP has not arrived yet.
# Phase B: send the re-park CUP + \e[?25h (what ConPTY forwards next) — caret
#          jumps back to the composer position.
set -u
read -r rows cols < <(stty size)
composer_row=$((rows - 2))
footer_row=$((rows - 1))

printf '\e[2J\e[H'
printf '\e[%d;1H› ' "$composer_row"
printf '\e[%d;1H  kimi_ranweiwei/kimi-for-coding' "$footer_row"   # LAST write -> cursor ends at col 32

# ---- Phase A (hold ~6s): exactly the captured frame tail ----
printf '\e[?25h\e[25l'
sleep 14

# ---- Phase B: the app's re-park arrives ----
printf '\e[%d;3H\e[?25h' "$composer_row"
sleep 4
